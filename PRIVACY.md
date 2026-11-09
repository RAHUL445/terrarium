# Privacy and data handling — source preview

The host stores productivity records locally in per-plugin/per-space SQLite
stores. A project-operated account or cloud service is not required. Local
records are not claimed to be encrypted at rest; platform secret storage
(Keychain/Android Keystore) protects credentials and keys, not every database.

Optional configured connections change where data goes:

- Paired-device sync transfers encrypted/signed records and packages; peers and
  transports can observe connection and transfer metadata. Optional Google Drive
  sync uses the user's Drive authorization and stores encrypted app data there.
- AI features send the selected report/content and instructions to the configured
  provider/endpoint. Local models can be selected; a remote provider receives
  those inputs. Provider policies are separate from this project.
- Voice capture keeps audio/transcripts locally and can use configured local
  transcription. Historical CRDT records/backups may retain audio or text after
  an active field is cleared. Complete erasure/retention controls remain pending.
- Plugin imports, configured Git URLs, model/tool downloads and opened links
  contact the selected external service. Installed plugins use host-enforced
  capability interfaces; signatures identify a publisher, not harmless behavior.
- OS notifications can expose titles/previews on screen. A universal preview
  preference remains pending. Exports/backups and recovery phrases are sensitive.

This is an engineering description of the source preview, not an app-store Data
safety declaration or a promise that remote providers never retain data. See
`docs/roadmap.md`, `SECURITY.md` and `docs/security/private-audio.md` for unfinished
controls. Use synthetic data when reporting issues; do not upload your database,
recovery phrase, credentials or private logs to a public issue.
