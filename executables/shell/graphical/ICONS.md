# Application icons

A shortcut may contain an optional versioned `icon` object alongside its
existing `icon_string` and `icon_color` fields. For example:

```json
{"version":1,"mode":"procedural","motif":"rings","monogram":"C",
 "background":"#4F46E5","foreground":"#FFFFFF","accent":"#A5B4FC","seed":17}
```

The available motifs are `rings`, `diagonal`, `grid`, `steps`, `orbit` and
`rays`. The monogram is at most two Unicode scalar values, colors are six-digit
RGB hex strings, and seed is a 32-bit unsigned integer. Missing fields use
fallback values; an invalid descriptor falls back to the existing icon fields
and a deterministic motif based on the shortcut file name. Shortcuts are capped
at 4096 bytes. The renderer uses LVGL primitives and no pixel buffer; it does
not explicitly allocate from the drawing callback; LVGL may allocate draw tasks.

The launcher copies the resolved icon to a bounded inherited environment value.
Built-in native windows and the Wasm host apply it to windows from the launched
task. Native applications may call `Window::set_icon_spec` to override it;
existing `set_icon(text, color)` and the Wasm guest call remain available. A
shortcut-provided launch descriptor takes precedence over that legacy shorthand;
`set_icon_spec` remains an explicit native override.
Third-party native code needs to opt into `task_launch_icon` and
`Window::apply_launch_spec` when creating windows. Rich vector recipes and a
new guest runtime override are future additive extensions; there is no SVG
interpreter or guest-supplied pointer kept by the host.
