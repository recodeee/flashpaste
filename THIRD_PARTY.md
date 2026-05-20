# Third-Party Notices

This file records third-party projects directly referenced by the FlashPaste
overlay work. FlashPaste itself is MIT licensed; see [`LICENSE`](LICENSE).

## Runtime Dependencies

### smithay-client-toolkit

- Project: [Smithay client-toolkit](https://github.com/Smithay/client-toolkit)
- Use: Wayland client and layer-shell plumbing for `flashpaste-overlayd`.
- License: MIT.
- Authors credited by the crate metadata: Elinor Berger, i509VCB, and Ashley
  Wulber. The license text in the crate also carries Victor Berger's copyright.

### cairo-rs, cairo-sys-rs, and pangocairo

- Project: [gtk-rs core bindings](https://github.com/gtk-rs/gtk-rs-core)
- Use: Rust bindings used by the optional overlay renderer.
- License: MIT.
- Authors credited by the crate metadata: The gtk-rs Project Developers.

### Cairo graphics library

- Project: [Cairo](https://www.cairographics.org/)
- Use: Underlying 2D graphics library used through the Rust Cairo bindings.
- License: LGPL-2.1 or MPL-1.1, at the user's option.
- Credit: Cairo authors and contributors.

## Reference Projects

### Gromit-MPX

- Project: [Gromit-MPX](https://github.com/bk138/gromit-mpx)
- Use: Reference for mature human-driven screen annotation behavior.
- License: GPL-2.0.
- Credit: Simon Budig, Christian Beier, Barak A. Pearlmutter, and the
  Gromit-MPX contributors listed in `references/gromit-mpx/AUTHORS`.
- Note: FlashPaste does not copy or vendor Gromit-MPX code.

### wayscriber

- Project: [wayscriber](https://github.com/devmobasa/wayscriber)
- Use: Reference for Wayland annotation UX, Smithay client-toolkit usage, Cairo
  rendering, and compositor-specific behavior.
- License: MIT.
- Credit: devmobasa and wayscriber contributors.
- Note: FlashPaste does not copy or vendor wayscriber code.
