Vendored dependencies used by the OS crate.

- `lwext4_rust`: copied from `https://github.com/elliott10/lwext4_rust` at
  `f9e3de7b0485429104e6b06ae0795e18f68ec957`, including its `c/lwext4`
  source tree so the build does not need `git submodule update`.
- `riscv`: copied from `https://github.com/rcore-os/riscv` at
  `11d43cf7cccb3b62a3caaf3e07a1db7449588f9a`.

These crates are referenced with `path` dependencies from `os/Cargo.toml` to
avoid network access during contest grading builds.
