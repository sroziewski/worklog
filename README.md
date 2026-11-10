# worklog

Suggests Jira worklogs from the day's git activity. At 17:00 on weekdays a window lists the
tickets you worked on with proposed hours; **Accept** logs the work in Jira, **Deny** dismisses it.

Rust + egui: one native executable, Windows 11 now, the same code builds on Linux.

## How it works

1. **Activity.** For every git repo under the configured roots (`WORKLOG_REPO_ROOTS` in `.env`,
   see [Local settings](#local-settings-env)), it collects your work of the day:
   - your commits on any branch, remote or tag (merges and stashes left out; a rebased or
     cherry-picked commit counts on the day it was *authored*),
   - branch checkouts from the reflog,
   - uncommitted files changed today (only when the day is today).

   A commit present in several repos counts once. "Your" means the configured
   `author_emails`, or `git config --global user.email` when none are set.
2. **Tickets.** Jira keys of the configured projects (`WORKLOG_PROJECT_KEYS`, e.g. `ABC`, `XYZ`) are taken from the
   commit subject, then the body, then the branch name — case-insensitive, so
   `feat/abc-1439-profile-select` → `ABC-1439`.
3. **Hours.** Each activity is credited the time since the previous one (a gap over 2h starts a
   new session, credited 30 min). The workday — **8h** by default, minus whatever you already
   logged in Jira that day — is split across tickets in proportion, in 15-minute steps:
   - one ticket → it gets the full 8h,
   - several tickets → the 8h are shared,
   - activity with no ticket key → shown at 0h with a dropdown of your open issues; it is
     logged only if you pick one and set hours.
4. **Window.** Always on top. Each row shows the ticket, its summary and status, the evidence
   (commit subjects, branches, files) and **editable hours**; untick a row to skip it.
   - **Accept** posts one worklog per ticked row, back to back from 09:00 (after anything
     already logged), with the commit subjects as the comment. Each row shows *logged* or
     *failed* (hover for the reason); **Retry** posts only the failed ones.
   - **Deny** closes without logging.

   Whether the window opens is decided by **what Jira shows for the day**, not by an earlier
   Accept: after accepting 4h of an 8h day, `worklog popup` (and the 17:00 run) opens again for
   the remaining 4h. It stays closed when the day is full or you clicked **Deny** for it.

   The "pick ticket…" dropdown has a project filter at the top — **All** and one entry per
   project, labelled with Jira's project name, e.g. **Alpha (ABC)** — to narrow your open issues
   to one system.

   **+ Add row** (under the table) adds another row while the ticked rows don't fill the
   workday yet: it starts with the hours still missing, you pick a ticket and adjust the hours;
   **×** removes it. Such rows are logged without a comment.
5. **Already logged.** Before opening, both `worklog popup` and the scheduled run ask Jira how
   much you logged that day — by hand or through the app; with a full workday (8h,
   `workday_minutes`) there, no window appears and the console/log says why. If Jira can't be
   reached it opens anyway. `--force` always opens. Worklogs are not stored locally on purpose:
   Jira is the only place that knows about time logged in the browser, so it is asked each time
   (~0.3 s).
6. **Missed days.** If the PC was off at 17:00, the scheduled run starts as soon as possible
   afterwards and asks about the day you missed (the latest weekday whose 17:00 has passed).

## Creating tickets from the app

When the work has no ticket yet, create it from the window:

1. Click **➕ New ticket…** — top right of the window, under the table, or under a row's
   **pick ticket…** box (that one opens with the row's project chosen and its lines ticked).
2. In the *New ticket* window choose the project, e.g. **Alpha (ABC)**. The list below
   shows only that project's work (by repo, `repo_projects`), grouped by row; tick the commits,
   branches and files the ticket is about — or a row's title to tick all its lines.
3. **✨ Propose tickets** sends the ticked lines, with what each one changed (the commit's
   message and diff, or the uncommitted file's diff), to the configured model in four steps, shown as
   they run:
   1. **analyst** — works out from the code what the work was for: the problem, the required
      behaviour, what must keep working, what is unclear; groups the lines into up to 3 tasks;
   2. **ticket writer** — a title naming the problem, **Opis** and **Kryteria akceptacji**,
      written as a task defined up front, not a change log;
   3. **tester** — **Przypadki testowe** (situation, action, expected result), only when there
      is behaviour to test;
   4. **reviewer** — strikes claims the analysis does not support and vague wording ("działa
      poprawnie", "jest pokryte testami").

   The description is put together as Jira wiki markup (`h3.` sections, `*` and `#` lists).
   If the tester or reviewer step fails, the tickets come without that step. Or use
   **+ Write one myself**.
4. Edit titles and descriptions, untick proposals you don't want, and click **Create N in ABC**.
   Tickets are created as a **Task** assigned to you, with any extra fields your projects
   require (`WORKLOG_TICKET_FIELDS`). Each proposal shows its new key (a link) or *failed*
   (hover for why).
5. New tickets appear at the top of every **pick ticket…** list. Each proposal says which rows
   it is for (the rows of the lines it covers); those rows pick it and get ticked, unless they
   already have a ticket picked. Set the hours and Accept as usual.

## Editing tickets created here

**🗂 My tickets** (top right of the window) lists the tickets created with **➕ New ticket** in
the last 7 days, newest first, with a project filter and a search on key and title. Each one was
stored when it was created, together with what it was drafted from: the ticked lines and their
diffs. Tickets made elsewhere are not listed; there is nothing to regenerate them from.

Click a ticket to load its current title and description from Jira, then:

- edit them by hand, or
- **✨ Regenerate** — the same four steps (analyst, writer, tester, reviewer) rewrite them from
  what the fields hold now and the stored lines and diffs. The result only fills the fields;
- **↺ Revert** brings back Jira's version;
- **💾 Save changes** writes the changed fields to Jira. If the ticket was changed in Jira since
  you opened it (in the browser, by someone else), Save refuses instead of overwriting it.

The new title then shows in the main window's rows and **pick ticket…** lists. Picking another
ticket with unsaved changes asks first; closing the window keeps them until the app closes.

Only the ticked lines go to the model: their text (commit subjects, branch and file names, repo
names) and, unless `send_code = false`, their diffs — up to 20000 characters each and 200000 in
all, lock files and similar left out. The model endpoint and key come from opencode's config
(`~/.config/opencode/opencode.json`, the provider named by `WORKLOG_LLM_PROVIDER`); override with the environment variables
`LLM_URL`, `LLM_API_KEY`, `LLM_MODEL`, or the `[tickets]` config section below.

## Install

Requires `git` on `PATH` and Rust (to build).

```powershell
cd D:\dev\my\worklog
cargo build --release
```

This produces two executables in `target\release\`:

| File | Purpose |
|---|---|
| `worklog.exe` | Command line: dry runs, manual popup, scheduling |
| `worklog-popup.exe` | The window the scheduler starts; opens no console window |

Keep them in the same folder. Then register the daily prompt:

```powershell
.\target\release\worklog.exe install     # Task Scheduler: \Worklog\DailyPrompt, Mon–Fri at popup_time
.\target\release\worklog.exe uninstall   # remove it
```

If you move the executables, run `install` again from the new location.

## Jira access

Jira Data Center with a Personal Access Token (Bearer). The credentials are looked up in this
order:

| Setting | 1st | 2nd | 3rd |
|---|---|---|---|
| `JIRA_URL` | environment variable | `jira_url` in config | ctx env file |
| `JIRA_TOKEN` | environment variable | ctx env file | — |

The ctx env file is `%APPDATA%\agents-context\.env` — the one the ctx project already uses, so
nothing extra is needed if ctx works. The token is never printed (`worklog config` shows it as
`<redacted>`).

## Commands

```powershell
worklog suggest                     # print today's proposals; logs nothing
worklog suggest --date 2026-11-09   # same for another day
worklog popup                       # open the window for today
worklog popup --date 2026-11-09     # ... for another day
worklog popup --force               # ... even if the day is full or was denied
worklog --check-weekly              # hours logged this week (Mon–Fri) per day and issue vs 8h/day
worklog --check-weekly --date 2026-11-03   # ... for the week holding that day
worklog config                      # show the effective config and Jira URL
worklog install | uninstall         # manage the scheduled prompt
worklog --config path\to\file.toml <command>   # use another config file
worklog -v suggest                  # also show every git command and Jira request with timings
worklog -q suggest                  # no progress lines, only warnings and the result
```

The first line of every run names the log file. Progress goes to the console (stderr) while it
works — Jira user, each repo scanned with its event count and time, what was already logged,
each suggestion. Repos are read 5 at a time; the window's loading line shows the step, and while
reading git how many repos are done and which ones are being read. A run takes a few seconds
to a minute, mostly git.

Test it with `suggest` first; it is read-only.

## Local settings (`.env`)

Your repos, Jira projects, Jira URL and LLM provider are not built in: they go in a `.env`
file, which git ignores, so they never reach the repository. Copy `.env.example` to `.env` and
fill it in. Put values holding `\` or `"` (paths, JSON) in single quotes, which keep them
literally:

```dotenv
# ';'-separated folders that are git repos or hold them
WORKLOG_REPO_ROOTS='D:\dev\alpha;D:\dev\xyz'
# ','-separated Jira project keys
WORKLOG_PROJECT_KEYS=ABC,XYZ
# project of the work in each root
WORKLOG_REPO_PROJECTS='D:\dev\alpha=ABC;D:\dev\xyz=XYZ'
JIRA_URL=https://jira.example.org
# provider entry in opencode.json, and the model
WORKLOG_LLM_PROVIDER=myllm
WORKLOG_LLM_MODEL=my-model
# extra Jira fields of new tickets, as JSON
WORKLOG_TICKET_FIELDS='{"customfield_10000":{"value":"development"}}'
```

The first `.env` found is used: the file `WORKLOG_ENV` names, then `%APPDATA%\worklog\.env`,
then a `.env` next to the executables or up to three folders above them (so the repo's own
`.env` works for `target\release\worklog.exe`). Every value can be left out; a key set in
`config.toml` wins over the `.env` one. Without project keys from either, the app stops with an
error saying where to set them.

## Configuration

Optional file: `%APPDATA%\worklog\config.toml` (Linux: `~/.config/worklog/config.toml`).
Every key is optional; unset ones come from `.env`, then from the built-in defaults below (the
values shown for the repos, projects and LLM are examples, the built-in ones are empty):

```toml
repo_roots = ['D:\dev\alpha', 'D:\dev\xyz']  # repos themselves, or folders holding them (2 levels deep)
author_emails = []              # empty = git config --global user.email
project_keys = ["ABC", "XYZ"]
workday_minutes = 480           # 8h
day_start = "09:00"             # first worklog starts here
popup_time = "17:00"            # used by `install`; re-run install after changing it
round_minutes = 15
max_gap_minutes = 120           # longer pause = new work session
session_start_minutes = 30      # credit for the first activity of a session
# ctx_env_file = 'C:\Users\you\AppData\Roaming\agents-context\.env'
# jira_url = "https://jira.example.org"

[repo_projects]                 # project of the work in each repo root (filters the New ticket window)
'D:\dev\alpha' = "ABC"
'D:\dev\xyz' = "XYZ"

[tickets]                       # creating tickets from the app
issue_type = "Task"
language = "Polish"             # language the model writes tickets in
llm_provider = "myllm"          # provider entry in opencode.json to borrow
llm_model = "my-model"
send_code = true                # send the diffs of the ticked lines; false = only their text
# llm_url = "https://llm.example.org/api"   # default: the provider's baseURL
# opencode_config = 'C:\Users\you\.config\opencode\opencode.json'

[tickets.fields]                # extra Jira fields for every new ticket (replaces WORKLOG_TICKET_FIELDS)
customfield_10000 = { value = "development" }   # e.g. a select field your Task type requires
```

Use single quotes for Windows paths in TOML. Unknown keys are rejected, so typos show up as
errors.

## Files

| Path | Content |
|---|---|
| `%APPDATA%\worklog\config.toml` | your settings (optional) |
| `%APPDATA%\worklog\state.json` | days answered (Accepted/Denied), last ~3 months; only a Deny keeps the window closed |
| `%APPDATA%\worklog\worklog.log` | log of every run, CLI and scheduled window, with debug detail (git commands, Jira requests and timings, worklogs posted, Accept/Deny); restarts after 2 MB; never contains the token |
| `%APPDATA%\worklog\last-error.txt` | why the scheduled window failed to open, if it did |
| `%APPDATA%\worklog\tickets.redb` | tickets created here in the last 7 days with the lines and diffs they were drafted from (redb database; older ones are removed when the window opens). One window at a time can use it; a second one works without **My tickets** |

The environment variables `WORKLOG_LOG`, `WORKLOG_STATE` and `WORKLOG_TICKETS` move the log, state and ticket history files
elsewhere (portable setups, tests).

To be asked again about a denied day, delete its line from `state.json`, or run
`worklog popup --date <day> --force`.

Example `worklog --check-weekly`:

```
Week 2026-11-09 – 2026-11-15 for jdoe

  Mon 2026-11-09       0m / 8h   missing 8h
  Tue 2026-11-10       4h / 8h   missing 4h ← today
  Wed 2026-11-11       0m / 8h   upcoming
  ...
By issue:
  ABC-1439            4h

Total 4h · target so far 16h (missing 12h) · full week 40h
```

## Troubleshooting

- **Anything odd:** look at `worklog.log` first, or rerun with `worklog -v suggest`.
- **No window at 17:00:** check `worklog.log` and `last-error.txt`, then Task Scheduler →
  `Worklog\DailyPrompt` → History. The task runs only while you are logged on.
- **"Jira refused the token":** the PAT expired or was revoked; put a new one in the ctx `.env`.
- **A ticket is missing:** its key must be in a commit message or branch name, and its project
  in `project_keys`. Otherwise pick it in the dropdown of the "no ticket key" row.
- **Hours look wrong:** they are estimates from commit timing — edit them before Accept.

## Linux

The same code builds with `cargo build --release`. `worklog install` writes a systemd user timer
(`~/.config/systemd/user/worklog.timer`, `OnCalendar=Mon..Fri`, `Persistent=true`) and enables
it. The service needs `DISPLAY` / `WAYLAND_DISPLAY` in the systemd user environment; most
desktops import them, otherwise add `systemctl --user import-environment DISPLAY WAYLAND_DISPLAY`
to your session startup. The Linux path has not been tested yet.

## Development

```powershell
cargo test --no-fail-fast    # ~430 tests; temp git repos and a local fake Jira, never the real one
cargo clippy --all-targets
```
