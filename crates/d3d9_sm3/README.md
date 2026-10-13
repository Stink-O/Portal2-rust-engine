# d3d9_sm3

Vendored from IW4L (Apache-2.0, see the repository NOTICE): D3D9 shader
bytecode token IR and WGSL emission. Used by `source_assets` to run Portal 2's
compiled shaders (`platform/shaders/fxc/*.vcs`).

## Local changes

- `bytecode.rs`: `TokenStream::parse` also accepts shader model 2 version
  tokens (`vs_2_0`, `ps_2_0`, `ps_2_b` = 2.1). SM2.x uses the same
  instruction-length encoding as SM3, and every Portal 2 world/model shader
  is `vs_2_0` / `ps_2_b`. Nothing else changed.
