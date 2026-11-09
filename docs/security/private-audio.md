# Private audio handling

## Implemented (2026-11-07)

The local decoder/model runs in an exclusive random `pp-speech-` temporary directory. Unix permissions are set to `0700` **at creation**, independent of the process umask. A `TempDir` guard removes the workspace after success, errors, or panic unwinding. Concurrent workspaces do not reuse names. Raw external decoder/model stderr is discarded; user-facing failure messages do not contain its transcripts, paths, or diagnostic text. Stdout is drained concurrently and capped at 64 KiB before JSON parsing. Existing 8 MiB input and 180-second decoded-audio limits remain.

Synthetic tests verify directory permissions, isolation, panic cleanup, diagnostic redaction, oversized output rejection and timeout termination. No recording, speech model or real user data is used by these tests.

## Retention and limits

Voice intentionally stores pending audio for transcription across devices; successful transcription clears the active row's audio field. Text/history is retained until the user changes or deletes it. This is **not secure erasure**: CRDT history, backups, previously downloaded data, and filesystem snapshots can retain older audio/text. Do not claim deleting a row destroys all copies.

Normal completion removes temporary decoder/model files. Forced termination, process abort, power loss, filesystem deletion failure, or descendants spawned by a custom configured executable can leave files. Automatic stale-directory sweeping is not implemented: safely identifying ownership and active workspaces is required before deleting anything. Unix directory access restrictions remain; encrypted storage/OS protection is still important. Windows ACL verification and physical Android capture/cache lifecycle testing remain open.

This milestone does not change reminder previews or claim all app/AI logs are redacted. A device-local notification preview preference, AI/network error redaction review, explicit capture-history retention controls and backup compaction/retention policy are separate work.

## AI error and network boundary follow-up

AI completion/model-list responses now share a 1 MiB read limit before JSON parsing. Provider HTTP error bodies are not read into persisted job errors; HTTP status categories remain useful for retry/authentication handling. Transport failures and malformed JSON/empty-text errors use fixed messages rather than echoing URLs, provider bodies or finish-reason strings. Automatic redirects are disabled: configure the provider's final base URL explicitly. This prevents a configured endpoint from redirecting an authenticated request elsewhere. Tests use synthetic local replies, including oversize and personal-data-looking error content. Model-generated successful text is intentionally returned to the user and can still contain sensitive material supplied in the prompt.

Scheduled-job failure notifications now use a generic body directing the user to Settings; raw plugin/provider failure strings stay out of the OS notification body. Schedule names, normal reminder content and successful notices still appear as before. A universal hide-preview setting remains pending, so notification permission should not be described as a complete lock-screen privacy control.
