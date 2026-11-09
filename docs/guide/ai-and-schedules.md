# AI, schedules and voice

## AI

Plugins that ask for the `ai` permission, such as Reports, and the Companion
assistant can generate text with an AI provider you choose. Plugins never see
the provider, the key or the network; the host makes the request for them.

Set it up in **Settings → AI**:

1. Turn on **Let plugins use AI**.
2. Pick a **Service**: OpenAI, OpenRouter, FastRouter, **Ollama (on this
   computer)**, or **Custom** for any OpenAI-compatible endpoint (set its
   **Base URL**).
3. Choose a **Model**, or use **Load models** to list what the service offers.
4. Paste an **API key** and choose **Save key** (not needed for Ollama).
5. Choose **Save**, then **Test connection**.

**Advanced** sets temperature, reply length, timeout, concurrent requests,
the largest prompt and a monthly request limit. **This month** shows usage.

The settings and the key sync to your paired devices. The key is stored
encrypted with the space key, so your devices can use it but plugins cannot
read it. Text you send goes to the provider you configured; with Ollama it
stays on your computer.

## Schedules

Plugins can run work on a schedule, such as a weekly review in Reports.
**Settings → Scheduled** lists every schedule and its **Recent runs**.

- Schedules sync to all your devices and run on whichever device is open when
  they fall due. Phones run them only while the app is open.
- **Pause all schedules** stops them on this device.
- Choose whether to be notified when a run fails or has something ready.

## Reminders

Plugins such as To-do and Habits schedule reminders as local notifications.
Each reminder can go to the phone, the desktop or both. Allow notifications
for Terrarium in your system settings if none appear.

## Voice transcription

The Voice plugin records audio or takes typed captures on any device.
Recordings are transcribed on a Mac with local speech-to-text, and nothing is
sent to a server. Transcription is off until you set it up; captures wait until
then.

It needs Apple silicon and:

- A Python virtual environment with `mlx` and `parakeet-mlx` installed.
- The 4-bit Parakeet v2 MLX checkpoint (a folder with `config.json` and
  `model.safetensors`).
- `ffmpeg`.

Then create `speech.json` in the app data folder
(`~/Library/Application Support/io.github.rahul445.terrarium/`):

```json
{
  "python": "/path/to/venv/bin/python",
  "model_dir": "/path/to/parakeet-v2-4bit",
  "ffmpeg": "/opt/homebrew/bin/ffmpeg"
}
```

Restart Terrarium. If any of the files are missing, transcription stays off.
Transcription runs with networking disabled and never downloads models.
