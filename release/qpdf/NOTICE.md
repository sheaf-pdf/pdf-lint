# qpdf, bundled

This archive's `qpdf/` holds qpdf 12.4.2, from its official release, <https://github.com/qpdf/qpdf/releases/tag/v12.4.2>: the files it needs to run, taken unmodified from the release's zip for this platform, whose SHA-256 `release/qpdf.sh` pins. pdf-lint runs it as a separate program, to decode streams and decrypt strings.

qpdf is licensed under the Apache License, Version 2.0, in `LICENSE.txt`. Its notice, with the parts it carries under other terms, is `QPDF-NOTICE.md`. Its source is <https://github.com/qpdf/qpdf/releases/download/v12.4.2/qpdf-12.4.2.tar.gz>.

What else qpdf's zip carries depends on the platform.

## Linux

qpdf's Linux zips are built on Ubuntu 22.04, and carry, beside `libqpdf`, these shared libraries from Ubuntu 22.04's packages. Each is its own file in `lib/`, which may be replaced with another build of the same library.

| File | Library | Version | License | Ubuntu source package |
|---|---|---|---|---|
| `libgnutls.so.30` | GnuTLS | 3.7.3 | LGPL-2.1-or-later | `gnutls28` 3.7.3-4ubuntu1.9 |
| `libnettle.so.8`, `libhogweed.so.6` | Nettle | 3.7.3 | LGPL-3.0-or-later or GPL-2.0-or-later | `nettle` 3.7.3-1build2 |
| `libtasn1.so.6` | libtasn1 | 4.18.0 | LGPL-2.1-or-later | `libtasn1-6` 4.18.0-4ubuntu0.2 |
| `libidn2.so.0` | libidn2 | 2.3.2 | LGPL-3.0-or-later or GPL-2.0-or-later | `libidn2` 2.3.2-2build1 |
| `libunistring.so.2` | libunistring | 1.0 | LGPL-3.0-or-later or GPL-2.0-or-later | `libunistring` 1.0-1 |
| `libp11-kit.so.0` | p11-kit | 0.24.0 | BSD-3-Clause | `p11-kit` 0.24.0-6ubuntu0.1 |
| `libffi.so.8` | libffi | 3.4.2 | MIT | `libffi` 3.4.2-4 |
| `libjpeg.so.8` | libjpeg-turbo | 2.1.2 | IJG, BSD-3-Clause and Zlib | `libjpeg-turbo` 2.1.2-0ubuntu1 |

Each package's copyright file from Ubuntu, with its authors and full terms, is in `linux/`, named for the source package, and so are the GNU licenses they refer to: `GPL-2`, `GPL-3`, `LGPL-2.1` and `LGPL-3`.

The corresponding source of each is Ubuntu's source package, at `https://launchpad.net/ubuntu/+source/<package>/<version>`: for GnuTLS, <https://launchpad.net/ubuntu/+source/gnutls28/3.7.3-4ubuntu1.9>. The versions are those Ubuntu 22.04 published when qpdf 12.4.2 was built (2026-09-26), and match the libraries' own version strings.

## macOS

qpdf's macOS zips carry `libqpdf` alone, statically linked with libjpeg and OpenSSL, and otherwise using libraries that are part of macOS (zlib among them). OpenSSL is licensed under the Apache License, Version 2.0, in `LICENSE.txt`. libjpeg's terms are in `JPEG.md`.

## Windows

qpdf's Windows zip is statically linked with its dependencies, zlib, libjpeg and OpenSSL, and carries the Microsoft Visual C++ runtime DLLs it needs (`msvcp140*.dll`, `vcruntime140*.dll`, `concrt140.dll`), which Microsoft lets applications redistribute. zlib's license asks only that its notice stay with its source. OpenSSL is licensed under the Apache License, Version 2.0, in `LICENSE.txt`. libjpeg's terms are in `JPEG.md`.
