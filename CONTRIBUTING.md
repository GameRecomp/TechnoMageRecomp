# Contributing

Thanks for helping. Please read this first. Rule 1 protects the project.

## Setup

```sh
git config core.hooksPath tools/hooks    # once per clone: audit before each commit, commit message check
```

## Rules

1. **No copyrighted material, anywhere.** That covers files, commit messages, issues, PRs and CI artifacts. It
   includes anything from `orig/` or `generated/`, disassembly or decompiled listings of the game, screenshots
   or text from the game, and code copied from projects whose license does not allow it (for example leaked
   SDKs). Describing *behaviour* in your own words (for example "the function at `0x4a1230` reads the level
   header") is fine and encouraged.
2. **No personal data or absolute machine paths** in committed files. Use paths relative to the repository root.
3. **Rust only.** No C, C++ or assembly sources, no C libraries built by us, no SDL. `tools/audit.sh` refuses C
   sources. Crates that wrap C libraries need a decision in the log of [docs/PLAN.md](docs/PLAN.md) first.
4. **Every source file starts with an SPDX identifier** within its first five lines:
   `SPDX-License-Identifier: MIT`. `tools/audit.sh` enforces this.
5. **Hashes, not files.** New builds are added by hash and size ([docs/TARGETS.md](docs/TARGETS.md) and the
   manifest).
6. **Keep it deterministic.** The recompiler output must not depend on the host, the time or the order of hash maps.
7. **Keep shared code general.** The recompiler crates are shared with the sister projects (TropicoRecomp,
   CulturesRecomp, HoMM4Recomp, SackedRecomp). Game-specific behaviour goes into `games/technomage/`, never into
   a special case in shared code, so fixes can travel between the repositories.
8. **Tests go with the change.** The lifter gets difftests, the HLE gets unit tests where it can be tested
   without the game, the manifest is covered by the consistency test.

## Style

- Rust: stable toolchain, `cargo fmt`, `cargo clippy --all-targets -- -D warnings`. `unsafe` only in the runtime's
  memory, executable-memory and thread modules and in the platform layer, each block with a `// SAFETY:` comment.
- HLE entry points are named after the API (`kernel32::CreateFileA`, `#[allow(non_snake_case)]`).
- Docs: English, short sentences, facts with sources. Open questions are marked **(open)** in
  [docs/RESEARCH.md](docs/RESEARCH.md).

## Commits and PRs

- Commit messages describe the change only. They do not mention AI tools, models, vendors or sessions, and they
  carry no attribution trailers for them. `tools/hooks/commit-msg` enforces this.
- One topic per PR. Reference the milestone (for example `M1: InstallShield 7 cabinets`).
- Update the status in [docs/PLAN.md](docs/PLAN.md) when a checklist item is done.
