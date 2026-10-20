# Vendored files

- `model-viewer.min.js`: Google's `<model-viewer>` web component 4.0.0, the 3D viewer on 3D model cards.
  - Source: https://cdn.jsdelivr.net/npm/@google/model-viewer@4.0.0/dist/model-viewer.min.js (fetched 2026-10-12).
  - License: Apache-2.0 per its package.json; the file's header also carries Google's BSD-3-Clause notice for bundled parts.
  - sha256 774edda21e1be2a0934e460ca5943af1fe3f88da130a9f98bd6a9d611576cacf
  - It is kept local so the app works offline. Compressed models (Draco/KTX2) need decoders from Google's CDN, which the
    studio's page policy (`connect-src 'self'`) blocks, so they do not display. Meshy GLBs load as delivered.
