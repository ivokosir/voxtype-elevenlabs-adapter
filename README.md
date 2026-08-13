# Voxtype ElevenLabs Adapter

A small, local compatibility server that lets [Voxtype](https://github.com/peteonrails/voxtype) use [ElevenLabs Scribe](https://elevenlabs.io/docs/overview/capabilities/speech-to-text).

```text
Voxtype -> localhost adapter -> ElevenLabs Scribe -> Voxtype -> wtype
```

Voxtype keeps handling the hotkey, microphone, status, notifications, and text insertion. This adapter only translates Voxtype's OpenAI-compatible transcription request into the ElevenLabs API format.

## Install

Requirements: Linux, Rust, Voxtype, and an ElevenLabs API key.

```bash
./install-user.sh
voxtype-elevenlabs-adapter set-key
```

The key is read without terminal echo and saved at `~/.config/voxtype-elevenlabs-adapter/api-key` with `0600` permissions. It is never placed in command-line arguments, Voxtype configuration, or logs.

Configure Voxtype:

```toml
engine = "whisper"

[whisper]
mode = "remote"
model = "scribe_v2"
language = "en"
translate = false
remote_endpoint = "http://127.0.0.1:17811"
remote_model = "scribe_v2"
remote_timeout_secs = 120
```

Existing compositor commands remain unchanged. For example, niri toggle mode:

```kdl
Mod+B { spawn "voxtype" "record" "toggle"; }
```

## Optional Luna cleanup mode

The installer also adds `voxtype-luna-cleanup`, which uses the Codex CLI and `gpt-5.6-luna` with low reasoning to remove spoken clutter. It requires a Codex CLI login (`codex login status`). No OpenAI API key is stored by this project.

Add two named Voxtype profiles:

```toml
[profiles.direct]
post_process_command = "/home/your-user/.local/bin/voxtype-copy-transcript"
post_process_timeout_ms = 5000

[profiles.luna]
post_process_command = "/home/your-user/.local/bin/voxtype-luna-cleanup"
post_process_timeout_ms = 95000
```

Then bind a second key in niri:

```kdl
Mod+B { spawn "/home/your-user/.local/bin/voxtype-mode-toggle" "direct"; }
Mod+G { spawn "/home/your-user/.local/bin/voxtype-mode-toggle" "luna"; }
```

Either key can start or stop the single active recording. The key used to stop decides the result: `Mod+B` keeps the ElevenLabs transcript unchanged, while `Mod+G` cleans it with Luna. Both results are typed with wtype and copied to the clipboard. Presses during transcription are ignored so they cannot start another recording.

The cleanup prompt is installed at `~/.config/voxtype-elevenlabs-adapter/transcript-cleanup-prompt.txt`, so it can be edited without rebuilding. If cleanup fails or times out, Voxtype outputs the original transcription.

Restart Voxtype after changing its configuration:

```bash
systemctl --user restart voxtype.service
```

## Check it

```bash
voxtype-elevenlabs-adapter status
systemctl --user status voxtype-elevenlabs-adapter.service
journalctl --user -u voxtype-elevenlabs-adapter.service
```

## Configuration

Environment variables are optional:

| Variable | Default | Purpose |
| --- | --- | --- |
| `ELEVENLABS_API_KEY` | credential file | Alternative key source |
| `VOXTYPE_ELEVENLABS_CREDENTIALS` | XDG config path | Override credential file |
| `VOXTYPE_ELEVENLABS_MODEL` | `scribe_v2` | ElevenLabs model |
| `VOXTYPE_ELEVENLABS_NO_VERBATIM` | `false` | Remove fillers and false starts |
| `VOXTYPE_ELEVENLABS_API_BASE` | official API | Testing or compatible endpoint |

The server only accepts localhost bind addresses. Audio is held in memory and is not saved by the adapter. ElevenLabs receives the audio for transcription.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The tests use a fake ElevenLabs server; they do not require a key or make paid API calls.

## License

MIT
