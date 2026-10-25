# Third-party notices

Process Corral itself is MIT-licensed (see [LICENSE](LICENSE)). It is built with the open-source Rust crates listed below. This file lists every crate that ends up in, or is used to build, the shipped programs, with its version, licence and source, followed by the full text of each licence used.

Generated from `cargo metadata` for `x86_64-pc-windows-msvc` (release configuration, dev-only dependencies excluded).

- **`process-corral.exe` (engine and service)** is built from the 24 crates marked **Engine** below. It contains none of the GUI-only crates.
- **`process-corral-gui.exe`** (built with `--features gui`) uses those plus the 136 crates marked **GUI only**, 160 in total.
- **Role**: `runtime` crates are normal dependencies (cargo also counts procedural-macro crates here, although they only run at compile time and are not linked into the programs); `build-time` crates are only used by build scripts, for example the one that embeds the icon, and are not linked into the programs.

## Choices where a crate offers several licences

Where a crate is licensed "A OR B" you may use it under either. One crate needs a note:

- **self_cell 1.3.0** is licensed `Apache-2.0 OR GPL-2.0-only`. **It is used under Apache-2.0.** The GPL option is not taken, and no GPL-licensed code is part of Process Corral. This crate is GUI only.

## Fonts

`process-corral-gui.exe` embeds fonts through `epaint_default_fonts` 0.37.0 (licence expression `(MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0`; GUI only). With the configuration used here (`eframe` with `default_fonts`), these are compiled into the executable:

- **Hack Regular**: MIT licence (Copyright 2018 Source Foundry Authors), derived from DejaVu (public domain) and Bitstream Vera Sans Mono (Bitstream Vera License). The notice that accompanies the font is reproduced below.
- **Ubuntu Light**: Ubuntu Font Licence 1.0 (full text below).
- **egui-icons**: a five-glyph subset of emoji-icon-font (MIT licence, Copyright (c) 2014 John Slegers) that the crate's authors made for egui.

`epaint_default_fonts` also contains Noto Emoji (SIL Open Font License 1.1) behind an optional feature (`monochrome_emoji_fonts`). This build does not enable that feature, so Noto Emoji is not compiled in. The OFL-1.1 text is included anyway because the crate's licence expression covers it.

**These fonts are compiled unmodified, as supplied by the crate, into `process-corral-gui.exe`.** (The egui-icons file is the subset supplied by that crate.) Process Corral does not alter, rename or sell them separately. The engine `process-corral.exe` contains no fonts.

### Notice shipped with Hack Regular

The work in the Hack project is Copyright 2018 Source Foundry Authors and licensed under the MIT License

The work in the DejaVu project was committed to the public domain.

Bitstream Vera Sans Mono Copyright 2003 Bitstream Inc. and licensed under the Bitstream Vera License with Reserved Font Names "Bitstream" and "Vera"
MIT License

Copyright (c) 2018 Source Foundry Authors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
BITSTREAM VERA LICENSE

Copyright (c) 2003 by Bitstream, Inc. All Rights Reserved. Bitstream Vera is a trademark of Bitstream, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy of the fonts accompanying this license ("Fonts") and associated documentation files (the "Font Software"), to reproduce and distribute the Font Software, including without limitation the rights to use, copy, merge, publish, distribute, and/or sell copies of the Font Software, and to permit persons to whom the Font Software is furnished to do so, subject to the following conditions:

The above copyright and trademark notices and this permission notice shall be included in all copies of one or more of the Font Software typefaces.

The Font Software may be modified, altered, or added to, and in particular the designs of glyphs or characters in the Fonts may be modified and additional glyphs or characters may be added to the Fonts, only if the fonts are renamed to names not containing either the words "Bitstream" or the word "Vera".

This License becomes null and void to the extent applicable to Fonts or Font Software that has been modified and is distributed under the "Bitstream Vera" names.

The Font Software may be sold as part of a larger software package but no copy of one or more of the Font Software typefaces may be sold by itself.

THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL BITSTREAM OR THE GNOME FOUNDATION BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM OTHER DEALINGS IN THE FONT SOFTWARE.

Except as contained in this notice, the names of Gnome, the Gnome Foundation, and Bitstream Inc., shall not be used in advertising or otherwise to promote the sale, use or other dealings in this Font Software without prior written authorization from the Gnome Foundation or Bitstream Inc., respectively. For further information, contact: fonts at gnome dot org.

### emoji-icon-font (source of the egui-icons subset)

MIT License

Copyright (c) 2014 John Slegers

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## Crates

| Crate | Version | Licence | Used in | Role | Source |
| --- | --- | --- | --- | --- | --- |
| accesskit | 0.25.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/AccessKit/accesskit |
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | GUI only | runtime | https://github.com/oyvindln/adler2 |
| ahash | 0.8.12 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/tkaitchuck/ahash |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/zakarumych/allocator-api2 |
| arrayvec | 0.7.8 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/bluss/arrayvec |
| ash | 0.38.0+1.3.281 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/ash-rs/ash |
| autocfg | 1.5.1 | Apache-2.0 OR MIT | GUI only | build-time | https://github.com/cuviper/autocfg |
| bit-set | 0.10.0 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/contain-rs/bit-set |
| bit-vec | 0.9.1 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/contain-rs/bit-vec |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/bitflags/bitflags |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT | GUI only | runtime | https://github.com/Lokathor/bytemuck |
| bytemuck_derive | 1.12.1 | Zlib OR Apache-2.0 OR MIT | GUI only | runtime | https://github.com/Lokathor/bytemuck |
| byteorder-lite | 0.1.0 | Unlicense OR MIT | GUI only | runtime | https://github.com/image-rs/byteorder-lite |
| cfg_aliases | 0.2.2 | MIT | GUI only | build-time | https://github.com/katharostech/cfg_aliases |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rust-lang/cfg-if |
| codespan-reporting | 0.13.1 | Apache-2.0 | GUI only | runtime | https://github.com/brendanzab/codespan |
| color | 0.3.3 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/linebender/color |
| crc32fast | 1.5.3 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/srijs/rust-crc32fast |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/crossbeam-rs/crossbeam |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/crossbeam-rs/crossbeam |
| cursor-icon | 1.2.0 | MIT OR Apache-2.0 OR Zlib | GUI only | runtime | https://github.com/rust-windowing/cursor-icon |
| document-features | 0.2.12 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/slint-ui/document-features |
| dpi | 0.1.2 | Apache-2.0 AND MIT | GUI only | runtime | https://github.com/rust-windowing/winit |
| ecolor | 0.37.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/emilk/egui |
| eframe | 0.37.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/emilk/egui/tree/main/crates/eframe |
| egui | 0.37.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/emilk/egui |
| egui-wgpu | 0.37.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/emilk/egui/tree/main/crates/egui-wgpu |
| egui-winit | 0.37.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/emilk/egui/tree/main/crates/egui-winit |
| either | 1.19.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rayon-rs/either |
| emath | 0.37.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/emilk/egui/tree/main/crates/emath |
| epaint | 0.37.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/emilk/egui/tree/main/crates/epaint |
| epaint_default_fonts | 0.37.0 | (MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0 | GUI only | runtime | https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts |
| equivalent | 1.0.3 | Apache-2.0 OR MIT | Engine | build-time | https://github.com/indexmap-rs/equivalent |
| euclid | 0.22.14 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/servo/euclid |
| fdeflate | 0.3.7 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/image-rs/fdeflate |
| fearless_simd | 0.4.1 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/linebender/fearless_simd |
| flate2 | 1.1.10 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rust-lang/flate2-rs |
| foldhash | 0.2.0 | Zlib | GUI only | runtime | https://github.com/orlp/foldhash |
| font-types | 0.12.7 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/googlefonts/fontations |
| gl_generator | 0.14.0 | Apache-2.0 | GUI only | build-time | https://github.com/brendanzab/gl-rs/ |
| glifo | 0.3.0 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/linebender/vello |
| glow | 0.17.0 | MIT OR Apache-2.0 OR Zlib | GUI only | runtime | https://github.com/grovesNL/glow |
| glutin_wgl_sys | 0.6.1 | Apache-2.0 | GUI only | runtime | https://github.com/rust-windowing/glutin |
| gpu-allocator | 0.28.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/Traverse-Research/gpu-allocator |
| guillotiere | 0.7.0 | MIT/Apache-2.0 | GUI only | runtime | https://github.com/nical/guillotiere |
| half | 2.7.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/VoidStarKat/half-rs |
| harfrust | 0.12.0 | MIT | GUI only | runtime | https://github.com/harfbuzz/harfrust |
| hashbrown | 0.16.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rust-lang/hashbrown |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 | Engine | build-time | https://github.com/rust-lang/hashbrown |
| image | 0.25.10 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/image-rs/image |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | Engine | build-time | https://github.com/indexmap-rs/indexmap |
| itertools | 0.15.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rust-itertools/itertools |
| itoa | 1.0.18 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/dtolnay/itoa |
| keyboard-types | 0.8.3 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rust-windowing/keyboard-types |
| khronos_api | 3.1.0 | Apache-2.0 | GUI only | build-time | https://github.com/brendanzab/gl-rs/ |
| khronos-egl | 6.0.0 | MIT/Apache-2.0 | GUI only | runtime | https://github.com/timothee-haudebourg/khronos-egl |
| kurbo | 0.13.1 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/linebender/kurbo |
| libc | 0.2.191 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rust-lang/libc |
| libloading | 0.8.9 | ISC | GUI only | runtime | https://github.com/nagisa/rust_libloading/ |
| libm | 0.2.16 | MIT | GUI only | runtime | https://github.com/rust-lang/compiler-builtins |
| linebender_resource_handle | 0.1.1 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/linebender/raw_resource_handle |
| litrs | 1.0.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/LukasKalbertodt/litrs |
| lock_api | 0.4.14 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/Amanieu/parking_lot |
| log | 0.4.34 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rust-lang/log |
| memchr | 2.8.3 | Unlicense OR MIT | Engine | runtime | https://github.com/BurntSushi/memchr |
| miniz_oxide | 0.8.9 | MIT OR Zlib OR Apache-2.0 | GUI only | runtime | https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 | GUI only | runtime | https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide |
| moxcms | 0.8.1 | BSD-3-Clause OR Apache-2.0 | GUI only | runtime | https://github.com/awxkee/moxcms.git |
| muda | 0.21.2 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/tauri-apps/muda |
| naga | 30.0.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/wgpu |
| naga-types | 30.0.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/wgpu |
| nohash-hasher | 0.2.0 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/paritytech/nohash-hasher |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rust-num/num-traits |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/matklad/once_cell |
| ordered-float | 5.5.0 | MIT | GUI only | runtime | https://github.com/reem/rust-ordered-float |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/Amanieu/parking_lot |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/Amanieu/parking_lot |
| peniko | 0.6.1 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/linebender/peniko |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/taiki-e/pin-project-lite |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 | GUI only | build-time | https://github.com/rust-lang/pkg-config-rs |
| png | 0.18.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/image-rs/image-png |
| pollster | 1.0.1 | Apache-2.0/MIT | GUI only | runtime | https://github.com/zesterer/pollster |
| polycool | 0.4.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/linebender/kurbo |
| presser | 0.3.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/EmbarkStudios/presser |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/dtolnay/proc-macro2 |
| profiling | 1.0.18 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/aclysma/profiling |
| pxfm | 0.1.30 | BSD-3-Clause OR Apache-2.0 | GUI only | runtime | https://github.com/awxkee/pxfm |
| quote | 1.0.47 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/dtolnay/quote |
| range-alloc | 0.1.5 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/range-alloc |
| raw-window-handle | 0.6.2 | MIT OR Apache-2.0 OR Zlib | GUI only | runtime | https://github.com/rust-windowing/raw-window-handle |
| read-fonts | 0.41.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/googlefonts/fontations |
| renderdoc-sys | 1.1.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/ebkalderon/renderdoc-rs |
| rustc-hash | 1.1.0 | Apache-2.0/MIT | GUI only | runtime | https://github.com/rust-lang-nursery/rustc-hash |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/rust-lang/rustc-hash |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/bluss/scopeguard |
| self_cell | 1.3.0 | Apache-2.0 OR GPL-2.0-only (used under Apache-2.0) | GUI only | runtime | https://github.com/Voultapher/self_cell |
| serde | 1.0.229 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/serde-rs/serde |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/serde-rs/serde |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/serde-rs/serde |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/serde-rs/json |
| serde_spanned | 1.1.2 | MIT OR Apache-2.0 | Engine | build-time | https://github.com/toml-rs/toml |
| simd-adler32 | 0.3.10 | MIT | GUI only | runtime | https://github.com/mcountryman/simd-adler32 |
| skrifa | 0.44.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/googlefonts/fontations |
| smallvec | 1.16.4 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/servo/rust-smallvec |
| smol_str | 0.2.2 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/rust-analyzer/smol_str |
| spirv | 0.4.0+sdk-1.4.341.0 | Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/rspirv |
| static_assertions | 1.1.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/nvzqz/static-assertions-rs |
| syn | 2.0.119 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/dtolnay/syn |
| syn | 3.0.9 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/dtolnay/syn |
| termcolor | 1.4.1 | Unlicense OR MIT | GUI only | runtime | https://github.com/BurntSushi/termcolor |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/dtolnay/thiserror |
| thiserror-impl | 2.0.21 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/dtolnay/thiserror |
| toml | 1.1.8+spec-1.1.0 | MIT OR Apache-2.0 | Engine | build-time | https://github.com/toml-rs/toml |
| toml_datetime | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 | Engine | build-time | https://github.com/toml-rs/toml |
| toml_parser | 1.1.5+spec-1.1.0 | MIT OR Apache-2.0 | Engine | build-time | https://github.com/toml-rs/toml |
| toml_writer | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | Engine | build-time | https://github.com/toml-rs/toml |
| tracing | 0.1.44 | MIT | GUI only | runtime | https://github.com/tokio-rs/tracing |
| tracing-core | 0.1.36 | MIT | GUI only | runtime | https://github.com/tokio-rs/tracing |
| tray-icon | 0.26.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/tauri-apps/tray-icon |
| type-map | 0.5.1 | MIT/Apache-2.0 | GUI only | runtime | https://github.com/kardeiz/type-map |
| unicode-general-category | 1.1.0 | Apache-2.0 | GUI only | runtime | https://github.com/yeslogic/unicode-general-category |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | Engine | runtime | https://github.com/dtolnay/unicode-ident |
| unicode-properties | 0.1.4 | MIT/Apache-2.0 | GUI only | runtime | https://github.com/unicode-rs/unicode-properties |
| unicode-segmentation | 1.13.3 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/unicode-rs/unicode-segmentation |
| unicode-width | 0.2.2 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/unicode-rs/unicode-width |
| uuid | 1.29.0 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/uuid-rs/uuid |
| vello_common | 0.2.0 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/linebender/vello |
| vello_cpu | 0.2.0 | Apache-2.0 OR MIT | GUI only | runtime | https://github.com/linebender/vello |
| version_check | 0.9.5 | MIT/Apache-2.0 | Engine | build-time | https://github.com/SergioBenitez/version_check |
| web-time | 1.1.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/daxpedda/web-time |
| wgpu | 30.0.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/wgpu |
| wgpu-core | 30.0.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/wgpu |
| wgpu-core-deps-windows-linux-android | 30.0.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/wgpu |
| wgpu-hal | 30.0.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/wgpu |
| wgpu-naga-bridge | 30.0.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/wgpu |
| wgpu-types | 30.0.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/gfx-rs/wgpu |
| winapi-util | 0.1.11 | Unlicense OR MIT | GUI only | runtime | https://github.com/BurntSushi/winapi-util |
| windows | 0.62.2 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-core | 0.62.2 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-future | 0.3.2 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/microsoft/windows-rs |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-result | 0.4.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | Engine | runtime | https://github.com/microsoft/windows-rs |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 | GUI only | runtime | https://github.com/microsoft/windows-rs |
| winit | 0.30.13 | Apache-2.0 | GUI only | runtime | https://github.com/rust-windowing/winit |
| winnow | 1.0.4 | MIT | Engine | build-time | https://github.com/winnow-rs/winnow |
| winresource | 0.1.34 | MIT | Engine | build-time | https://github.com/BenjaminRi/winresource |
| xml-rs | 0.8.29 | MIT | GUI only | build-time | https://github.com/kornelski/xml-rs |
| zerocopy | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT | GUI only | runtime | https://github.com/google/zerocopy |
| zerocopy-derive | 0.8.63 | BSD-2-Clause OR Apache-2.0 OR MIT | GUI only | runtime | https://github.com/google/zerocopy |
| zlib-rs | 0.6.8 | Zlib | GUI only | runtime | https://github.com/trifectatechfoundation/zlib-rs |
| zmij | 1.0.23 | MIT | Engine | runtime | https://github.com/dtolnay/zmij |

## Licence texts

Each distinct licence appears once. Where a text contains a copyright line, it belongs to the crate named in the note under the heading; every other crate under the same licence has its own copyright line, which you can find in its source repository linked above.

### MIT

Each crate carries its own copyright line (see its repository); the grant text is the same.

```text
MIT License

Copyright (c) <year> <copyright holders of the crate>

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

### Apache-2.0

The appendix placeholders are part of the standard text; each crate names its own copyright holder in its repository.

```text
Apache License
                        Version 2.0, January 2004
                     http://www.apache.org/licenses/

TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

1. Definitions.

   "License" shall mean the terms and conditions for use, reproduction,
   and distribution as defined by Sections 1 through 9 of this document.

   "Licensor" shall mean the copyright owner or entity authorized by
   the copyright owner that is granting the License.

   "Legal Entity" shall mean the union of the acting entity and all
   other entities that control, are controlled by, or are under common
   control with that entity. For the purposes of this definition,
   "control" means (i) the power, direct or indirect, to cause the
   direction or management of such entity, whether by contract or
   otherwise, or (ii) ownership of fifty percent (50%) or more of the
   outstanding shares, or (iii) beneficial ownership of such entity.

   "You" (or "Your") shall mean an individual or Legal Entity
   exercising permissions granted by this License.

   "Source" form shall mean the preferred form for making modifications,
   including but not limited to software source code, documentation
   source, and configuration files.

   "Object" form shall mean any form resulting from mechanical
   transformation or translation of a Source form, including but
   not limited to compiled object code, generated documentation,
   and conversions to other media types.

   "Work" shall mean the work of authorship, whether in Source or
   Object form, made available under the License, as indicated by a
   copyright notice that is included in or attached to the work
   (an example is provided in the Appendix below).

   "Derivative Works" shall mean any work, whether in Source or Object
   form, that is based on (or derived from) the Work and for which the
   editorial revisions, annotations, elaborations, or other modifications
   represent, as a whole, an original work of authorship. For the purposes
   of this License, Derivative Works shall not include works that remain
   separable from, or merely link (or bind by name) to the interfaces of,
   the Work and Derivative Works thereof.

   "Contribution" shall mean any work of authorship, including
   the original version of the Work and any modifications or additions
   to that Work or Derivative Works thereof, that is intentionally
   submitted to Licensor for inclusion in the Work by the copyright owner
   or by an individual or Legal Entity authorized to submit on behalf of
   the copyright owner. For the purposes of this definition, "submitted"
   means any form of electronic, verbal, or written communication sent
   to the Licensor or its representatives, including but not limited to
   communication on electronic mailing lists, source code control systems,
   and issue tracking systems that are managed by, or on behalf of, the
   Licensor for the purpose of discussing and improving the Work, but
   excluding communication that is conspicuously marked or otherwise
   designated in writing by the copyright owner as "Not a Contribution."

   "Contributor" shall mean Licensor and any individual or Legal Entity
   on behalf of whom a Contribution has been received by Licensor and
   subsequently incorporated within the Work.

2. Grant of Copyright License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   copyright license to reproduce, prepare Derivative Works of,
   publicly display, publicly perform, sublicense, and distribute the
   Work and such Derivative Works in Source or Object form.

3. Grant of Patent License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   (except as stated in this section) patent license to make, have made,
   use, offer to sell, sell, import, and otherwise transfer the Work,
   where such license applies only to those patent claims licensable
   by such Contributor that are necessarily infringed by their
   Contribution(s) alone or by combination of their Contribution(s)
   with the Work to which such Contribution(s) was submitted. If You
   institute patent litigation against any entity (including a
   cross-claim or counterclaim in a lawsuit) alleging that the Work
   or a Contribution incorporated within the Work constitutes direct
   or contributory patent infringement, then any patent licenses
   granted to You under this License for that Work shall terminate
   as of the date such litigation is filed.

4. Redistribution. You may reproduce and distribute copies of the
   Work or Derivative Works thereof in any medium, with or without
   modifications, and in Source or Object form, provided that You
   meet the following conditions:

   (a) You must give any other recipients of the Work or
       Derivative Works a copy of this License; and

   (b) You must cause any modified files to carry prominent notices
       stating that You changed the files; and

   (c) You must retain, in the Source form of any Derivative Works
       that You distribute, all copyright, patent, trademark, and
       attribution notices from the Source form of the Work,
       excluding those notices that do not pertain to any part of
       the Derivative Works; and

   (d) If the Work includes a "NOTICE" text file as part of its
       distribution, then any Derivative Works that You distribute must
       include a readable copy of the attribution notices contained
       within such NOTICE file, excluding those notices that do not
       pertain to any part of the Derivative Works, in at least one
       of the following places: within a NOTICE text file distributed
       as part of the Derivative Works; within the Source form or
       documentation, if provided along with the Derivative Works; or,
       within a display generated by the Derivative Works, if and
       wherever such third-party notices normally appear. The contents
       of the NOTICE file are for informational purposes only and
       do not modify the License. You may add Your own attribution
       notices within Derivative Works that You distribute, alongside
       or as an addendum to the NOTICE text from the Work, provided
       that such additional attribution notices cannot be construed
       as modifying the License.

   You may add Your own copyright statement to Your modifications and
   may provide additional or different license terms and conditions
   for use, reproduction, or distribution of Your modifications, or
   for any such Derivative Works as a whole, provided Your use,
   reproduction, and distribution of the Work otherwise complies with
   the conditions stated in this License.

5. Submission of Contributions. Unless You explicitly state otherwise,
   any Contribution intentionally submitted for inclusion in the Work
   by You to the Licensor shall be under the terms and conditions of
   this License, without any additional terms or conditions.
   Notwithstanding the above, nothing herein shall supersede or modify
   the terms of any separate license agreement you may have executed
   with Licensor regarding such Contributions.

6. Trademarks. This License does not grant permission to use the trade
   names, trademarks, service marks, or product names of the Licensor,
   except as required for reasonable and customary use in describing the
   origin of the Work and reproducing the content of the NOTICE file.

7. Disclaimer of Warranty. Unless required by applicable law or
   agreed to in writing, Licensor provides the Work (and each
   Contributor provides its Contributions) on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
   implied, including, without limitation, any warranties or conditions
   of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
   PARTICULAR PURPOSE. You are solely responsible for determining the
   appropriateness of using or redistributing the Work and assume any
   risks associated with Your exercise of permissions under this License.

8. Limitation of Liability. In no event and under no legal theory,
   whether in tort (including negligence), contract, or otherwise,
   unless required by applicable law (such as deliberate and grossly
   negligent acts) or agreed to in writing, shall any Contributor be
   liable to You for damages, including any direct, indirect, special,
   incidental, or consequential damages of any character arising as a
   result of this License or out of the use or inability to use the
   Work (including but not limited to damages for loss of goodwill,
   work stoppage, computer failure or malfunction, or any and all
   other commercial damages or losses), even if such Contributor
   has been advised of the possibility of such damages.

9. Accepting Warranty or Additional Liability. While redistributing
   the Work or Derivative Works thereof, You may choose to offer,
   and charge a fee for, acceptance of support, warranty, indemnity,
   or other liability obligations and/or rights consistent with this
   License. However, in accepting such obligations, You may act only
   on Your own behalf and on Your sole responsibility, not on behalf
   of any other Contributor, and only if You agree to indemnify,
   defend, and hold each Contributor harmless for any liability
   incurred by, or claims asserted against, such Contributor by reason
   of your accepting any such warranty or additional liability.

END OF TERMS AND CONDITIONS
```

### Zlib

The copyright line shown is zlib-rs's; other Zlib crates use their own.

```text
(C) 2024 Trifecta Tech Foundation 

This software is provided 'as-is', without any express or implied
warranty. In no event will the authors be held liable for any damages
arising from the use of this software.

Permission is granted to anyone to use this software for any purpose,
including commercial applications, and to alter it and redistribute it
freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not
   claim that you wrote the original software. If you use this software
   in a product, an acknowledgment in the product documentation would be
   appreciated but is not required.

2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.

3. This notice may not be removed or altered from any source distribution.
```

### ISC

The copyright line shown is libloading's.

```text
Copyright © 2015, Simonas Kazlauskas

Permission to use, copy, modify, and/or distribute this software for any purpose with or without
fee is hereby granted, provided that the above copyright notice and this permission notice appear
in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
```

### 0BSD

The copyright line shown is adler2's.

```text
Copyright (C) Jonas Schievink <jonasschievink@gmail.com>

Permission to use, copy, modify, and/or distribute this software for
any purpose with or without fee is hereby granted.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN
AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT
OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```

### Unlicense

```text
This is free and unencumbered software released into the public domain.

Anyone is free to copy, modify, publish, use, compile, sell, or
distribute this software, either in source code form or as a compiled
binary, for any purpose, commercial or non-commercial, and by any
means.

In jurisdictions that recognize copyright laws, the author or authors
of this software dedicate any and all copyright interest in the
software to the public domain. We make this dedication for the benefit
of the public at large and to the detriment of our heirs and
successors. We intend this dedication to be an overt act of
relinquishment in perpetuity of all present and future rights to this
software under copyright law.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

For more information, please refer to <http://unlicense.org/>
```

### BSD-2-Clause

Text as shipped by zerocopy (Fuchsia authors); other BSD-2-Clause crates use their own copyright line.

```text
Copyright 2019 The Fuchsia Authors.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### BSD-3-Clause

Text as shipped by moxcms; pxfm uses the same licence with its own copyright line.

```text
Copyright (c) Radzivon Bartoshyk. All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

1.  Redistributions of source code must retain the above copyright notice, this
    list of conditions and the following disclaimer.

2.  Redistributions in binary form must reproduce the above copyright notice,
    this list of conditions and the following disclaimer in the documentation
    and/or other materials provided with the distribution.

3.  Neither the name of the copyright holder nor the names of its
    contributors may be used to endorse or promote products derived from
    this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### Unicode-3.0

Applies to the Unicode data tables in unicode-ident.

```text
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 1991-2023 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.
```

### OFL-1.1 (SIL Open Font License 1.1)

See "Fonts" above.

```text
This Font Software is licensed under the SIL Open Font License,
Version 1.1.

This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL

-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font
creation efforts of academic and linguistic communities, and to
provide a free and open framework in which fonts may be shared and
improved in partnership with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply to
any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software
components as distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to,
deleting, or substituting -- in part or in whole -- any of the
components of the Original Version, by changing formats or by porting
the Font Software to a new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed,
modify, redistribute, and sell modified and unmodified copies of the
Font Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components, in
Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the
corresponding Copyright Holder. This restriction only applies to the
primary font name as presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created using
the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

### Ubuntu-font-1.0 (Ubuntu Font Licence 1.0)

See "Fonts" above.

```text
-------------------------------
UBUNTU FONT LICENCE Version 1.0
-------------------------------

PREAMBLE
This licence allows the licensed fonts to be used, studied, modified and
redistributed freely. The fonts, including any derivative works, can be
bundled, embedded, and redistributed provided the terms of this licence
are met. The fonts and derivatives, however, cannot be released under
any other licence. The requirement for fonts to remain under this
licence does not require any document created using the fonts or their
derivatives to be published under this licence, as long as the primary
purpose of the document is not to be a vehicle for the distribution of
the fonts.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this licence and clearly marked as such. This may
include source files, build scripts and documentation.

"Original Version" refers to the collection of Font Software components
as received under this licence.

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to
a new environment.

"Copyright Holder(s)" refers to all individuals and companies who have a
copyright ownership of the Font Software.

"Substantially Changed" refers to Modified Versions which can be easily
identified as dissimilar to the Font Software by users of the Font
Software comparing the Original Version with the Modified Version.

To "Propagate" a work means to do anything with it that, without
permission, would make you directly or secondarily liable for
infringement under applicable copyright law, except executing it on a
computer or modifying a private copy. Propagation includes copying,
distribution (with or without modification and with or without charging
a redistribution fee), making available to the public, and in some
countries other activities as well.

PERMISSION & CONDITIONS
This licence does not grant any rights under trademark law and all such
rights are reserved.

Permission is hereby granted, free of charge, to any person obtaining a
copy of the Font Software, to propagate the Font Software, subject to
the below conditions:

1) Each copy of the Font Software must contain the above copyright
notice and this licence. These can be included either as stand-alone
text files, human-readable headers or in the appropriate machine-
readable metadata fields within text or binary files as long as those
fields can be easily viewed by the user.

2) The font name complies with the following:
(a) The Original Version must retain its name, unmodified.
(b) Modified Versions which are Substantially Changed must be renamed to
avoid use of the name of the Original Version or similar names entirely.
(c) Modified Versions which are not Substantially Changed must be
renamed to both (i) retain the name of the Original Version and (ii) add
additional naming elements to distinguish the Modified Version from the
Original Version. The name of such Modified Versions must be the name of
the Original Version, with "derivative X" where X represents the name of
the new work, appended to that name.

3) The name(s) of the Copyright Holder(s) and any contributor to the
Font Software shall not be used to promote, endorse or advertise any
Modified Version, except (i) as required by this licence, (ii) to
acknowledge the contribution(s) of the Copyright Holder(s) or (iii) with
their explicit written permission.

4) The Font Software, modified or unmodified, in part or in whole, must
be distributed entirely under this licence, and must not be distributed
under any other licence. The requirement for fonts to remain under this
licence does not affect any document created using the Font Software,
except any version of the Font Software extracted from a document
created using the Font Software may only be distributed under this
licence.

TERMINATION
This licence becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM OTHER
DEALINGS IN THE FONT SOFTWARE.
```
