#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

cargo build --release --locked --manifest-path "$project_dir/Cargo.toml"
install -Dm755 "$project_dir/target/release/voxtype-elevenlabs-adapter" \
  "$HOME/.local/bin/voxtype-elevenlabs-adapter"
install -Dm755 "$project_dir/scripts/voxtype-luna-cleanup" \
  "$HOME/.local/bin/voxtype-luna-cleanup"
install -Dm644 "$project_dir/config/transcript-cleanup-prompt.txt" \
  "$HOME/.config/voxtype-elevenlabs-adapter/transcript-cleanup-prompt.txt"
install -Dm644 "$project_dir/packaging/voxtype-elevenlabs-adapter.service" \
  "$HOME/.config/systemd/user/voxtype-elevenlabs-adapter.service"

systemctl --user daemon-reload
systemctl --user enable --now voxtype-elevenlabs-adapter.service

printf 'Installed. Run: voxtype-elevenlabs-adapter set-key\n'
