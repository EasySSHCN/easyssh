# EasySSH Free Promotion · International Pack (2026-11)

> Site: https://easyssh.cn · GitHub: https://github.com/EasySSHCN/easyssh
> Pitch: Free & open-source (MIT) portable SSH/SFTP terminal for Windows. No install — unzip and run; sessions travel with the folder. Rust + Tauri 2 + russh, 100% original code, SecureCRT-style workflow.

---

## 1. Show HN (news.ycombinator.com)

**Title**: `Show HN: EasySSH – Open-source portable SSH terminal for Windows (Rust/Tauri)`

**Text**:
Hi HN! We built EasySSH because every portable Windows machine we touched lacked a proper SSH client: PuTTY is fine but session management is painful, and the good commercial clients (SecureCRT, XShell) are neither free nor portable.

EasySSH is a single-folder app: unzip, run, and your sessions/keys/settings travel with the folder (USB drive, synced dir, whatever). No registry writes, no admin rights.

Highlights:
- Multi-tab terminal + split-pane SFTP with resumable transfers
- Port forwarding, SOCKS5/HTTP proxy, ProxyJump (up to 3 hops)
- Passwords encrypted at rest (AES-256-GCM, optional Argon2id master password)
- Update channel verified with Ed25519 signatures
- Stack: Rust + Tauri 2 + russh + xterm.js, fully open source (MIT)

Site (downloads + SHA256 checksums): https://easyssh.cn
Source: https://github.com/EasySSHCN/easyssh

Happy to answer questions about the russh integration or the Tauri packaging.

*Tips: post Tue–Thu 8–10 AM ET; reply to every comment in the first 4 hours; don't crosspost links.*

---

## 2. Reddit

**Subreddits**: r/sysadmin (show what it solves), r/commandline, r/rust (the Tauri/russh engineering angle), r/opensource, r/Tauri

**r/sysadmin template**:
Title: `Open-source portable SSH client for Windows that carries its session folder with it (MIT)`

Body: We maintain a small fleet of jump boxes and vendor laptops where we can't install software. EasySSH (MIT, Rust/Tauri) runs from any folder — sessions, keys, and settings live inside it. Multi-tab, SFTP with resume, ProxyJump chains, port forwarding. Site with checksums: https://easyssh.cn — source at github.com/EasySSHCN/easyssh. Feedback welcome, especially on the key-agent workflow.

*Note: r/sysadmin bans self-promo without disclosure — keep it low-key, answer questions, don't spam multiple subs the same day.*

---

## 3. Product Hunt

**Tagline**: `Portable open-source SSH terminal — unzip & go`
**Description**: EasySSH is a free, MIT-licensed SSH/SFTP terminal for Windows. No installer, no registry: sessions and settings live in one folder you can carry anywhere. Multi-tab terminal, resumable SFTP, port forwarding, ProxyJump, encrypted secrets. Built with Rust + Tauri 2.
**Maker comment**: story of why SecureCRT-style workflow + portability didn't exist in open source, and how russh made a pure-Rust SSH stack possible.
**Gallery**: 5 images — hero, multi-tab terminal, SFTP queue, forwarding settings, dark theme.

---

## 4. AlternativeTo (alternativeto.net)

Submit as alternative to: **PuTTY, SecureCRT, XShell, MobaXterm, Termius, Solar-PuTTY**.
Listing copy: `EasySSH is a free open-source (MIT) portable SSH/SFTP terminal for Windows. It runs without installation from a single folder, keeping sessions with it. Features multi-tab terminal, SFTP transfers with resume, port forwarding, SOCKS5/HTTP proxy, ProxyJump, encrypted stored credentials and Ed25519-verified updates.`
Tags: `ssh-client, sftp, portable, open-source, free, windows`.

## 5. Slant / StackShare / LibHunt / Awesome lists

- Submit PR to `awesome-tauri` (Apps section) and `awesome-ssh` — both accept apps; PR text: one line + site + repo.
- LibHunt: submit under Tauri/Rust categories.

## 6. Software directories (free listings)

- **Softpedia** "Free Submit" (softpedia.com/dynamic/add-program) — no account needed historically; use the description above, category: `Programming » Other Programming Files` or `Network Tools » SSH`.
- **MajorGeeks / Filepuma / Softpedia** review queues — expect days-to-weeks latency.
- **Fossies / OSS Insight** pick up repos automatically once stars grow.

## 7. Dev.to / Medium article (funnel for international devs)

Title: `Building a portable Windows SSH terminal with Rust, Tauri 2 and russh`
Structure: same outline as the Chinese Juejin pack; cross-link repo and site. Dev.to has good Google indexing for "tauri ssh client" long-tail queries.

---

## Two-week rollout (international)

| Week | Action |
|---|---|
| 1 | Show HN (Tue or Wed) · r/rust post · awesome-list PRs |
| 2 | Product Hunt launch (Tue–Thu) · AlternativeTo listings · Dev.to article · r/sysadmin discussion |
