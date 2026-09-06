#!/bin/bash
# Run the poste.nvim Lua (plenary) test suite.
# Isolated XDG dirs so headless runs never touch (or get blocked by) the
# user's real cache/state. Rust-side tests: `cargo test`.
set -e

cd "$(dirname "$0")/.."

export XDG_CACHE_HOME="${XDG_CACHE_HOME:-/tmp/poste-test-cache}"
export XDG_DATA_HOME="${XDG_DATA_HOME:-/tmp/poste-test-data}"
export XDG_STATE_HOME="${XDG_STATE_HOME:-/tmp/poste-test-state}"

if [ -z "$PLENARY_PATH" ]; then
  for dir in \
    "$HOME/.local/share/nvim/lazy/plenary.nvim" \
    "$HOME/.local/share/nvim/site/pack/packer/start/plenary.nvim" \
    "$HOME/.config/nvim/plugged/plenary.nvim" \
    "$HOME/.config/nvim/lazy/plenary.nvim"; do
    if [ -d "$dir" ]; then
      PLENARY_PATH="$dir"
      break
    fi
  done
fi

if [ -z "$PLENARY_PATH" ] || [ ! -d "$PLENARY_PATH" ]; then
  echo "Error: plenary.nvim not found."
  echo "Set PLENARY_PATH env var or install it:"
  echo "  git clone --depth 1 https://github.com/nvim-lua/plenary.nvim ~/.local/share/nvim/lazy/plenary.nvim"
  exit 1
fi

echo "Running poste.nvim Lua tests (PLENARY_PATH=$PLENARY_PATH)..."

nvim --headless \
  -u tests/minimal_init.lua \
  -c "set rtp+=$PLENARY_PATH" \
  -c "set rtp+=." \
  -c "runtime plugin/plenary.vim" \
  -c "PlenaryBustedDirectory tests/poste/ {minimal_init = 'tests/minimal_init.lua'}" \
  -c "qa!"
