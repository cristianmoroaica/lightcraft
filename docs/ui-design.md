# UI design notes: interaction feedback and controls

How LightCraft's own widgets look and respond, and why. This covers the widgets painted in
`crates/ui-egui/src/widgets.rs`, the tokens in `crates/ui-egui/src/theme.rs` and a few hand-painted
controls in the panels. Layout (panel widths, row heights, positions) is out of scope here: a
restyle never moves a widget, so automation ids, rects and screenshots stay comparable.

## Principles (2026 desktop practice)

- **Every interactive element answers the pointer.** Hover, press, drag and keyboard focus each
  get a distinct, visible state (WCAG 2.2 SC 2.4.7 *Focus Visible*; Material 3 *state layers*).
- **State layers instead of new colours.** Hover and press add a translucent veil (`Tokens::veil`:
  white on the dark themes, black on Silver / Paper) over
  whatever the widget already shows (about 8 % hover, about 13 % pressed, following Material 3's
  hover 8 % / pressed 10 % / dragged 16 % scale, tuned for a dark chrome). One helper,
  `widgets::state_layer`, keeps that consistent across buttons, tool icons, the top bar, the
  rating/flag pill, the filter bar and the source list.
- **Motion that reads but never waits.** State changes ease over 120 ms (`STATE_ANIM`; desktop
  guidance puts hover/press transitions at 100–150 ms). Nothing animates position or size of the
  layout. Widgets don't grow on hover (`expansion = 0`), so nothing shifts under the pointer.
- **Focus rings for keyboard users only.** A 2 px ring in a lighter accent (`Tokens::focus`,
  #4C9BFF: 4.7:1 against the chrome, above the 3:1 asked by WCAG 2.2 SC 2.4.13 *Focus
  Appearance* and SC 1.4.11 *Non-text Contrast*), drawn 1 px outside the control while it has
  keyboard focus (`widgets::focus_ring`).
- **Softer, consistent geometry.** Corner radii are 6 pt for controls, 8 pt for menus and
  popups and 10 pt for windows. Pill-shaped elements (bottom bar) stay fully round.
- **Restraint for a photo app.** The photo is the content: chrome stays neutral grey, accent
  colour only marks *selected* or *active* (selected buttons, open flyouts, focus, the dragged
  slider's glow), never decoration.

## Sliders

Research: Material 3 (2023–2025) moved to thicker rounded tracks with an *active track* fill and
a handle that reacts to press. Design-system benchmarks list default / hover / focus / dragging /
disabled states, with "hover: thumb expands or shows a halo; dragging: value read-out and an
enlarged thumb; disabled: reduced opacity". NN/g and others ask that sliders show their value and
offer a precise alternative (LightCraft already has arrow-key nudges, ⇧ fine-drag and
double-click reset).

What LightCraft does (same row height and track rect as before):

| State | Look |
| --- | --- |
| Rest | 4 pt rounded rail; plain tracks fill from the origin (0 for bipolar controls such as Exposure, else the minimum) to the thumb; gradient tracks (Temp, Tint, HSL bands) keep their colours as a rounded pill. Solid 12 pt light thumb with a hairline outline and soft shadow, so it separates from any gradient. The value is dimmed while it sits at the default and brighter once edited. |
| Hover (row) | Label and value brighten. The thumb grows 1 pt and turns white with a 10 % white halo. A small dark notch marks the default position (double-click returns there). Cursor: ↔. |
| Drag | Accent glow halo, slightly larger thumb, value read-out on a pill so the changing number is easy to follow. Cursor: grabbing. |
| Keyboard focus | 2 pt focus ring around the thumb. |
| Disabled | Rail, fill and thumb at reduced contrast. Label and value use the disabled text colour. |

## Buttons, segmented controls

Rounded 6 pt fill with a hairline border. Hover brightens the border and adds the state layer.
Press deepens the veil. A selected segment or active toggle takes an accent-tinted fill and
border instead of a slightly lighter grey, so "on" is unmistakable. Focus ring when focused.

## Icon buttons (tool strip, top bar, bottom bar)

A 32 pt rounded square appears under the icon on hover (state layer) and the glyph eases from
the icon grey to full text colour. The active tool keeps its filled square and white glyph.

## Section headers and flyout rows

Section headers get a faint full-width veil on hover, and the chevron brightens with the pointer.
Flyout rows (Curve, Color Mixer…) have a 6 pt rounded inset with a border that brightens on hover.
When open, their icon tile turns accent-tinted, matching the "accent = open/selected" rule.

## Rating stars

Hovering previews the rating a click would set: stars up to the pointer light up. The hovered star
also sits on the same rounded state layer as the icon buttons.

## Text fields (search)

8 pt radius. On hover the border and fill lift slightly. On focus the field gets the focus-colour
border plus a soft 3 pt halo outside it.

## Grid and filmstrip cells

Hover lifts the cell background towards the selected tone (animated). Selection and the active
photo keep their existing white/grey outlines.

## Tokens touched

`track` (rail), `track_fill` (new: active part of plain tracks), `thumb` / `thumb_hover`
(brighter on dark themes, for solid thumbs), `focus` (new; #4C9BFF on dark themes, #0A62D0 on light),
`veil` (new). Every value is set for all four themes. egui visuals: radii, hovered/active strokes,
`expansion = 0`, circle handles and trailing fill for any stock egui slider, a focus-coloured
text cursor, `animation_time` 0.12 s.

**Padding without moving anything.** egui's `button_padding`, `menu_margin` and `window_margin` stay
as they were: changing them resizes stock buttons, menus and dialogs, and the layout is fixed. The
breathing room comes from inside the existing rects instead: hover pills and state layers are inset
or expanded around their content (dropdown pill 5×2 pt, slider value pill 5×1 pt, focus rings 2–3 pt
outside the control), and nothing grows on hover (`expansion = 0`).

## Sources

- W3C, *WCAG 2.2*: SC 1.4.11 Non-text Contrast, 2.4.7 Focus Visible, 2.4.13 Focus Appearance, 2.5.8 Target Size (Minimum).
- Material Design 3: *States* (state layers), *Sliders* (2023 update and Expressive slider sizes), *Motion* (easing and duration tokens).
- Flutter, *Updated Material 3 Slider* (track gap, thumb width change on press).
- Setproduct, *Slider UI design guide*; Toyota Europe design system, *Slider*; designsystems.surf, *Slider blueprint*.
- UXPin, *Button States Explained: The Complete Design Guide for 2026*.
