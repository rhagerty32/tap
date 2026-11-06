#!/bin/bash

# Required parameters:
# @raycast.schemaVersion 1
# @raycast.title Open Tap team
# @raycast.mode silent
# @raycast.packageName Tap

# Optional parameters:
# @raycast.icon ⚡️
# @raycast.description Open Tap so you can hold a teammate's key

# Raycast hotkeys are set in Raycast, not in this file.
# Bind this command to Hyper plus another key, then hold the letter
# you assigned that person in Tap → Settings → Team members.

if ! open "tap://keys"; then
  echo "Couldn't open Tap. Launch Tap once so macOS learns the tap:// link." >&2
  exit 1
fi
