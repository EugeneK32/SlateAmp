# Changelog

## 0.9.0 (experimental)
- Persist named EQ presets and migrate gain-only configuration from older releases.
- Add Pop, Electronic, Jazz, Classical, Acoustic and Treble Boost presets.
- Remove duplicate EQ hover/focus decorations, retaining a keyboard-focus indicator.
- Add real-audio Mirror and Radial spectrum visualizers.

## 0.8.0 — Experimental responsive layout

- Compact: the familiar player / playlist / controls stacked vertically.
- Wide windows (880+ logical px): player and transport at left, track browser at right.
- Expanded windows (1480+ logical px): a third, functional music-folder sidebar.
- Clamp the maximum content width to 1600 px to keep 4K fullscreen usable.
- Reparent existing GTK widgets on layout changes; audio, track rows, scroll position,
  MPRIS and keyboard shortcuts keep their existing state.
- Visualizer display now emphasizes 45 Hz–12 kHz instead of allocating the last
  bar to a nearly silent 17 kHz band; analysis threshold lowered to -110 dB
  so quiet upper harmonics are preserved, with a genuine silence gate.
- No changes to the GStreamer playback pipeline or previously saved settings.

# Changelog

## 0.7.0 — Settings redesign

- Replaced the modal GTK dialog with an independent native libadwaita preferences window.
- Preferences always follow the GNOME appearance setting, independent of SlateAmp skins; player theme changes no longer alter the global libadwaita color scheme.
- Grouped options into Appearance and Library & playback pages; no unnecessary instructions or footer button.
- Removed the show/hide playlist button and its automatic window resizing. The queue, music and radio panel is always visible.
- Kept all audio, playlist, radio and settings persistence behavior from 0.6.0.
- Existing settings.json remains compatible; unused `playlist_expanded` is ignored.


## 0.6.0

- Added the new lightning-bolt app icon and improved GNOME desktop installation.
- Saved window geometry across sessions.
- Improved audio spectrum frequency grouping and display compensation.
- Added `slateampctl` for targeted media-key control and MPRIS diagnostics.

## 0.5.0

- Added MPRIS player integration.
- Fixed playlist scroll position when switching tracks.
- Expanded the themes (including Adwaita Light/Dark).

## 0.4.0

- Separated internet radio playback from the music queue.
- Added Space keyboard shortcut to play/pause.
- Reordered Music and Radio tabs and added more themes.

## 0.3.0

- Added music folder browsing, RU/EN interface, themes, visualizer variants,
  queue persistence, equalizer presets and shuffle/repeat controls.

## 0.2.0

- Redesigned the initial large UI into a compact player.

## 0.1.0

- Initial GTK4/GStreamer player with internet radio, queue and spectrum.
