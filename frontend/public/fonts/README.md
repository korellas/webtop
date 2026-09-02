# Fonts

IBM Plex Sans and IBM Plex Mono, latin subsets, four faces.

    ibm-plex-sans-latin-400-normal.woff2
    ibm-plex-sans-latin-600-normal.woff2
    ibm-plex-mono-latin-400-normal.woff2
    ibm-plex-mono-latin-600-normal.woff2

Copyright 2019 IBM Corp. Licensed under the SIL Open Font License 1.1, whose
full text is in `LICENSE.txt` beside these files — OFL redistribution requires
the licence to travel with the fonts, and these are redistributed rather than
fetched.

That is the point of them being here. webtop is a LAN tool, and a dashboard
whose typography depends on the uplink of the machine it monitors loses its
typography exactly when someone is looking at it. No CDN: Vite copies `public/`
into `dist/`, `build.rs` embeds `dist/`, and the binary serves its own faces.

Subsets came from the `@fontsource/ibm-plex-{sans,mono}` packages. `@font-face`
declarations are at the top of `frontend/src/app.css`; the type scale that uses
them is `docs/design-guide.md` §2.
