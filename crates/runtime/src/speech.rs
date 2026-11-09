//! Local speech-to-text through a Parakeet (MLX) worker process, configured
//! with a prepared virtualenv, a 4-bit Parakeet v2
//! checkpoint and ffmpeg. Audio is decoded to 16 kHz mono WAV in a private
//! temporary directory that is removed afterwards; nothing is downloaded,
//! stored or logged.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};

const MAX_AUDIO: usize = 8 * 1024 * 1024;
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(30);
const WORKER_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_WORKER_OUTPUT: usize = 64 * 1024;

/// One transcription at a time per process.
static BUSY: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpeechConfig {
    pub python: PathBuf,
    pub model_dir: PathBuf,
    pub ffmpeg: PathBuf,
}

impl SpeechConfig {
    pub fn ready(&self) -> bool {
        self.python.is_file()
            && self.ffmpeg.is_file()
            && self.model_dir.join("config.json").is_file()
            && self.model_dir.join("model.safetensors").is_file()
    }
}

const WORKER: &str = include_str!("../speech/transcribe.py");

fn run(mut command: Command, timeout: Duration, what: &str) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // External decoder/model diagnostics may contain transcripts or paths.
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| format!("{what} could not start"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{what} output unavailable"))?;
    // Drain concurrently so a full pipe cannot stall an otherwise valid worker.
    // Closing at the bound also prevents unbounded diagnostic output allocation.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take((MAX_WORKER_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() <= timeout => {
                std::thread::sleep(Duration::from_millis(50))
            }
            outcome => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(if outcome.is_err() {
                    format!("{what} could not complete")
                } else {
                    format!("{what} timed out")
                });
            }
        }
    };
    let output = reader
        .join()
        .map_err(|_| format!("{what} output unavailable"))?
        .map_err(|_| format!("{what} output unavailable"))?;
    if !status?.success() {
        return Err(format!("{what} failed"));
    }
    if output.len() > MAX_WORKER_OUTPUT {
        return Err(format!("{what} output exceeds its limit"));
    }
    Ok(output)
}

// TempDir creates an exclusive, unpredictable owner-only directory on Unix.
// Its guard removes files on every ordinary error and panic unwind.
fn scratch() -> Result<tempfile::TempDir, String> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("pp-speech-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir()
        .map_err(|_| "private speech workspace unavailable".to_string())
}

/// Decodes base64 audio and returns the transcript.
pub fn transcribe(config: &SpeechConfig, audio_base64: &str) -> Result<String, String> {
    if !config.ready() {
        return Err("local speech-to-text is not set up on this device".into());
    }
    if audio_base64.len() > MAX_AUDIO.div_ceil(3) * 4 {
        return Err("audio exceeds its 8 MiB limit".into());
    }
    let audio = B64
        .decode(audio_base64.trim())
        .map_err(|_| "audio is not valid base64".to_string())?;
    if audio.is_empty() || audio.len() > MAX_AUDIO {
        return Err("audio must be between 1 byte and 8 MiB".into());
    }
    let format = match &audio[..audio.len().min(12)] {
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'A', b'V', b'E'] => "wav",
        [0x1a, 0x45, 0xdf, 0xa3, ..] => "webm",
        [b'O', b'g', b'g', b'S', ..] => "ogg",
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => "mp4",
        [b'I', b'D', b'3', ..] | [0xff, 0xfb | 0xf3 | 0xf2, ..] => "mp3",
        _ => return Err("unsupported audio format; use WAV, M4A, WebM, Ogg or MP3".into()),
    };
    let _gate = BUSY
        .try_lock()
        .map_err(|_| "another transcription is running; try again shortly".to_string())?;

    let workspace = scratch()?;
    let dir = workspace.path();
    let result = (|| {
        let input = dir.join(format!("input.{format}"));
        std::fs::File::create(&input)
            .and_then(|mut f| f.write_all(&audio))
            .map_err(|e| e.to_string())?;
        let wav = dir.join("audio.wav");
        let mut ffmpeg = Command::new(&config.ffmpeg);
        ffmpeg
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-f",
                format,
                "-i",
            ])
            .arg(&input)
            .args(["-t", "180", "-ac", "1", "-ar", "16000", "-f", "wav"])
            .arg(&wav);
        run(ffmpeg, FFMPEG_TIMEOUT, "audio decoding")?;
        let worker = dir.join("transcribe.py");
        std::fs::write(&worker, WORKER).map_err(|e| e.to_string())?;
        let mut python = Command::new(&config.python);
        python
            .arg(&worker)
            .arg(&config.model_dir)
            .arg(&wav)
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1");
        let out = run(python, WORKER_TIMEOUT, "speech model")?;
        let reply: serde_json::Value = serde_json::from_slice(&out)
            .map_err(|_| "speech model returned no transcript".to_string())?;
        reply["text"]
            .as_str()
            .map(|t| t.trim().to_string())
            .ok_or_else(|| "speech model returned no transcript".to_string())
    })();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_is_exclusive_private_and_cleaned_on_unwind() {
        let one = scratch().unwrap();
        let two = scratch().unwrap();
        assert_ne!(one.path(), two.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(one.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let path = one.path().to_owned();
        let _ = std::panic::catch_unwind(|| {
            let _guard = one;
            std::fs::write(path.join("audio.wav"), b"synthetic private audio").unwrap();
            panic!("synthetic worker failure");
        });
        assert!(!path.exists());
        assert!(two.path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn worker_output_is_bounded_and_diagnostics_are_redacted() {
        let mut failed = Command::new("/bin/sh");
        failed.args(["-c", "echo PRIVATE_TRANSCRIPT >&2; exit 1"]);
        assert_eq!(
            run(failed, Duration::from_secs(2), "speech model").unwrap_err(),
            "speech model failed"
        );
        let mut large = Command::new("/bin/sh");
        large.args(["-c", "head -c 65537 /dev/zero"]);
        assert!(run(large, Duration::from_secs(2), "speech model")
            .unwrap_err()
            .contains("limit"));
        let mut slow = Command::new("/bin/sleep");
        slow.arg("2");
        assert!(run(slow, Duration::from_millis(10), "speech model")
            .unwrap_err()
            .contains("timed out"));
    }
}
