# Log Book Git

`log-book-git` is a small Rust CLI that pulls your GitLab activity and writes it out as a Markdown report grouped by week and day.

It is useful if you want a personal work log, a monthly activity summary, or a draft document for status updates.

## What It Collects

- Commits authored by your GitLab user
- Issues you created
- Merge requests you created
- Comments you wrote on issues and merge requests
- Commit diffs, embedded in collapsible Markdown sections

## Quick Start

### 1. Clone and build

```bash
git clone <your-fork-or-repo-url>
cd log-book-git
cargo build --release
```

The compiled binary will be available at `target/release/log-book-git`.

### 2. Create a GitLab access token

Create a personal access token in GitLab with these scopes:

- `read_api`
- `read_user`
- `read_repository`

### 3. Export the token

```bash
export GITLAB_TOKEN="your-gitlab-token"
```

### 4. Generate a report

For a specific month:

```bash
./target/release/log-book-git --month 01/2025
```

For the last 60 days:

```bash
./target/release/log-book-git --days 60
```

By default the report is written to `activity_report.md`.

## CLI Reference

```bash
log-book-git --help
```

Available options:

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `--token` | `-t` | GitLab personal access token | `GITLAB_TOKEN` |
| `--url` | `-U` | GitLab base URL | `https://gitlab.com` |
| `--output` | `-o` | Output Markdown path | `activity_report.md` |
| `--month` | `-m` | Month to fetch in `MM/YYYY` format | unset |
| `--days` | `-d` | Number of trailing days to fetch when `--month` is not used | `30` |

## Common Examples

Use a self-hosted GitLab instance:

```bash
./target/release/log-book-git \
  --url "https://gitlab.mycompany.com" \
  --month 12/2024
```

Write the report into the repo's `log_books_month` directory:

```bash
./target/release/log-book-git \
  --month 01/2025 \
  --output log_books_month/01_2025.md
```

Pass the token directly instead of using an environment variable:

```bash
./target/release/log-book-git \
  --token "your-gitlab-token" \
  --days 30
```

## Output Format

The generated file is plain Markdown and looks roughly like this:

```md
# GitLab Activity Report

Generated on: 2026-03-10 09:00:00 UTC

## Week: January 20, 2025 to January 26, 2025

### Monday, January 20, 2025

#### 🚀 Commit `abc1234` - [project-name](https://gitlab.com/group/project)

**Fix authentication bug**

[View Commit](https://gitlab.com/group/project/-/commit/abc1234)

<details>
<summary>Code Changes</summary>

**src/auth.rs** (modified)

```diff
- old code
+ new code
```

</details>

---
```

## Notes and Limitations

- The tool uses GitLab's REST API and may take a while on accounts with many projects.
- Comment collection is a second pass over updated issues and merge requests, so it can add noticeable API traffic.
- Large diffs are truncated in the Markdown output to keep reports readable.
- If `--month` is set, it takes precedence over `--days`.

## Development

Run the project without building a release binary:

```bash
cargo run -- --month 01/2025
```

Format and check the code locally:

```bash
cargo fmt
cargo check
```

## License

MIT
