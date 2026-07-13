# Compact Controls Design

## Goal

Keep buttons and drop-down fields visually compact and consistent across every settings page.

## Design

- Use a fixed 32 px height for all standard buttons and combo boxes.
- Set a fixed height and disable vertical stretching so Slint layouts cannot distribute spare space into the controls.
- Keep navigation items and specialized status elements unchanged because they are not form controls.
- Preserve existing widths, labels, colors, callbacks, and interaction behavior.

## Verification

- Compile the Slint UI and Rust application.
- Open each settings page and confirm buttons and combo boxes render at 32 px.
- Confirm the controls remain clickable and text stays vertically centered.
