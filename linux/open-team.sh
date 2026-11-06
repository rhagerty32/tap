#!/bin/bash

# Opens the Tap team list. Bind this in your desktop's keyboard settings
# when Tap cannot grab a global shortcut itself (most non-GNOME Wayland sessions).
# Person keys still work only while that list is open.

if ! xdg-open "tap://keys"; then
  echo "Couldn't open Tap. Launch Tap once so the tap:// link is registered." >&2
  exit 1
fi
