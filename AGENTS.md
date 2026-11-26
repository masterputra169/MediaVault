# AGENTS.md — MediaVault Coding Agent Instructions

## Mission
Build the complete local-only, single-user MediaVault application specified in `PRD.md`. Implement working software end-to-end, not a landing page or mock downloader.

## Must-read
Read `PRD.md` fully before modifying code. When requirements conflict, prioritize: (1) security/privacy, (2) functional download flow, (3) correctness/testability, (4) UI polish.

## Scope
- One local user, no auth or public hosting.
- Next.js App Router + TypeScript + Tailwind frontend.
- Python 3.11+ FastAPI backend; yt-dlp, FFmpeg, SQLite.
- Bind to 127.0.0.1 by default; no telemetry or cloud APIs.
- Build features FR-01 through FR-08 as P0, then FR-09 polish. FR-10 is out of scope.
- Don't implement DRM/paywall/login bypass or private-account scraping.

## Coding discipline
1. Inspect current repository and dependencies first; don't overwrite unrelated files.
2. Plan implementation in small checkpoints, then **actually execute** through MVP without stopping at scaffolding.
3. Keep extractors, job orchestration, filesystem operations, and API routes as separately testable modules.
4. Use `subprocess` argument arrays, never user-controlled shell commands. A running job must be cancellable and cleaned up.
5. Validate URL hostnames and redirect-related risk, prevent SSRF, path traversal, arbitrary file reads/deletes, and cross-origin mutations.
6. Keep structured API errors; no raw stack traces or credentials in responses/logs.
7. Store files in a configured local folder and job data in SQLite; create directories as needed.
8. Use migrations, predictable project-root-relative paths, and safe restarts.
9. All UI values must come from real backend responses; dummy data is for tests only.
10. Use stable compatible package versions; check official dependency docs when installation fails. Do not claim broad platform compatibility without verification.
11. Add automated tests for input validation, state changes, cancellation, file safety, and API. Do not make core tests require live internet.
12. Add Windows PowerShell and Linux/WSL scripts, accurate README, .env.example, and .gitignore.
13. In frontend, validate state transitions, handle null metadata, show disabled/loading and error states, and ensure keyboard accessibility.
14. Run frontend lint/typecheck/build, backend tests, and smoke tests if feasible. Do not report tests as passed unless executed.
15. Before finalizing, compare actual behavior to the DoD checklist at the end of PRD.

## Execution output required
At completion report:
- What was implemented (with file paths)
- Commands actually run and pass/fail results
- How to run on Windows and WSL2
- Where downloads and SQLite data are saved
- Any known limitations, especially real-site extraction not tested
- What's still pending if any DoD items aren't satisfied

## Avoid
- UI-only mock; fake progress; hardcoded media results
- Public `0.0.0.0` listen by default
- Redis/PostgreSQL/Docker requirement for MVP
- Browser extension, login, cookies import, DRM bypass, bulk playlist
- Automatically executing package update commands from the UI
- Exposing raw yt-dlp info dictionaries or sensitive source URLs
