# @rebuildup/skill-graph

Agent Skills discovery and recommendation CLI with opt-in runtime hooks
for Claude Code, Codex, and OpenCode.

## Install

```bash
npm install -g @rebuildup/skill-graph   # or: bun add -g / pnpm add -g
```

Platform binaries ship as optional dependencies — no postinstall
downloads or builds. Requires Node.js >= 14.18 for the launcher shim.

## Usage

```bash
skill-graph --help
skill-graph adapter enable claude-code   # opt-in hook installation
```

Prebuilt binaries: Linux x86_64/ARM64 (glibc), macOS x86_64/ARM64.
musl and Windows are not supported (use WSL2 on Windows).

Source: https://github.com/rebuildup/skill-graph
