use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use clap::Parser;
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Write as _;

const DEFAULT_GITLAB_URL: &str = "https://gitlab.com";
const DEFAULT_OUTPUT_PATH: &str = "activity_report.md";
const PAGE_SIZE: u32 = 100;
const DIFF_LINE_LIMIT: usize = 100;
const NOTE_PREVIEW_LIMIT: usize = 500;

#[derive(Parser)]
#[command(name = "log-book-git")]
#[command(about = "Fetch GitLab activity for a month and generate a markdown report")]
struct Args {
    /// GitLab personal access token
    #[arg(short, long, env = "GITLAB_TOKEN")]
    token: String,

    /// GitLab base URL
    #[arg(short = 'U', long, default_value = DEFAULT_GITLAB_URL)]
    url: String,

    /// Output markdown file path
    #[arg(short, long, default_value = DEFAULT_OUTPUT_PATH)]
    output: String,

    /// Month to fetch in MM/YYYY format (e.g. 01/2025 for January 2025)
    #[arg(short, long)]
    month: Option<String>,

    /// Number of days to fetch (ignored if --month is provided)
    #[arg(short, long, default_value_t = 30)]
    days: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct Project {
    id: i64,
    name: String,
    #[allow(dead_code)]
    path_with_namespace: String,
    web_url: String,
}

#[derive(Debug, Deserialize)]
struct User {
    username: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct Commit {
    id: String,
    short_id: String,
    title: String,
    message: String,
    authored_date: DateTime<Utc>,
    web_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommitDiff {
    new_path: String,
    diff: String,
    new_file: bool,
    renamed_file: bool,
    deleted_file: bool,
}

#[derive(Debug, Deserialize)]
struct Issue {
    iid: i64,
    title: String,
    description: Option<String>,
    state: String,
    created_at: DateTime<Utc>,
    web_url: String,
}

#[derive(Debug, Deserialize)]
struct MergeRequest {
    iid: i64,
    title: String,
    description: Option<String>,
    state: String,
    created_at: DateTime<Utc>,
    #[allow(dead_code)]
    merged_at: Option<DateTime<Utc>>,
    web_url: String,
    source_branch: String,
    target_branch: String,
}

#[derive(Debug, Deserialize)]
struct Note {
    body: String,
    author: NoteAuthor,
    created_at: DateTime<Utc>,
    system: bool,
}

#[derive(Debug, Deserialize)]
struct NoteAuthor {
    username: String,
}

struct GitLabClient {
    base_url: String,
    token: String,
    client: reqwest::blocking::Client,
}

impl GitLabClient {
    fn new(base_url: &str, token: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            client: reqwest::blocking::Client::new(),
        }
    }

    fn get<T: for<'de> Deserialize<'de>>(&self, endpoint: &str) -> Result<T> {
        let url = format!("{}/api/v4{}", self.base_url, endpoint);
        let response = self
            .client
            .get(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .send()
            .with_context(|| format!("Failed to fetch {endpoint}"))?;

        if !response.status().is_success() {
            anyhow::bail!(
                "API request failed for {}: {} - {}",
                endpoint,
                response.status(),
                response.text().unwrap_or_default()
            );
        }

        response.json().context("Failed to parse response")
    }

    fn get_paginated<T: for<'de> Deserialize<'de>>(&self, endpoint: &str) -> Result<Vec<T>> {
        let mut all_items = Vec::new();
        let mut page = 1;

        loop {
            let separator = if endpoint.contains('?') { "&" } else { "?" };
            let url = format!(
                "{}/api/v4{}{}page={page}&per_page={PAGE_SIZE}",
                self.base_url, endpoint, separator
            );

            let response = self
                .client
                .get(&url)
                .header("PRIVATE-TOKEN", &self.token)
                .send()
                .with_context(|| format!("Failed to fetch {endpoint}"))?;

            if !response.status().is_success() {
                if response.status().as_u16() == 404 {
                    break;
                }

                anyhow::bail!(
                    "API request failed for {}: {} - {}",
                    endpoint,
                    response.status(),
                    response.text().unwrap_or_default()
                );
            }

            let next_page = response
                .headers()
                .get("x-next-page")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);

            let items: Vec<T> = response.json().context("Failed to parse response")?;
            if items.is_empty() {
                break;
            }

            all_items.extend(items);

            match next_page {
                Some(next_page) => {
                    page = next_page
                        .parse()
                        .context("Invalid x-next-page header from GitLab API")?;
                }
                None => break,
            }
        }

        Ok(all_items)
    }

    fn get_current_user(&self) -> Result<User> {
        self.get("/user")
    }

    fn get_user_projects(&self) -> Result<Vec<Project>> {
        self.get_paginated("/projects?membership=true")
    }

    fn get_project_commits_by_author(
        &self,
        project_id: i64,
        author: &str,
        since: &str,
        until: &str,
    ) -> Result<Vec<Commit>> {
        self.get_paginated(&format!(
            "/projects/{project_id}/repository/commits?author={}&since={since}&until={until}",
            urlencoding::encode(author),
        ))
    }

    fn get_commit_diff(&self, project_id: i64, sha: &str) -> Result<Vec<CommitDiff>> {
        self.get(&format!(
            "/projects/{project_id}/repository/commits/{sha}/diff",
        ))
    }

    fn get_project_issues_by_author(
        &self,
        project_id: i64,
        author: &str,
        since: &str,
        until: &str,
    ) -> Result<Vec<Issue>> {
        self.get_paginated(&format!(
            "/projects/{project_id}/issues?author_username={}&created_after={since}&created_before={until}&scope=all",
            urlencoding::encode(author),
        ))
    }

    fn get_project_merge_requests_by_author(
        &self,
        project_id: i64,
        author: &str,
        since: &str,
        until: &str,
    ) -> Result<Vec<MergeRequest>> {
        self.get_paginated(&format!(
            "/projects/{project_id}/merge_requests?author_username={}&created_after={since}&created_before={until}&scope=all",
            urlencoding::encode(author),
        ))
    }

    fn get_project_issues_updated_in_range(
        &self,
        project_id: i64,
        since: &str,
        until: &str,
    ) -> Result<Vec<Issue>> {
        self.get_paginated(&format!(
            "/projects/{project_id}/issues?updated_after={since}&updated_before={until}&scope=all",
        ))
    }

    fn get_project_mrs_updated_in_range(
        &self,
        project_id: i64,
        since: &str,
        until: &str,
    ) -> Result<Vec<MergeRequest>> {
        self.get_paginated(&format!(
            "/projects/{project_id}/merge_requests?updated_after={since}&updated_before={until}&scope=all",
        ))
    }

    fn get_issue_notes(&self, project_id: i64, issue_iid: i64) -> Result<Vec<Note>> {
        self.get_paginated(&format!("/projects/{project_id}/issues/{issue_iid}/notes"))
    }

    fn get_mr_notes(&self, project_id: i64, mr_iid: i64) -> Result<Vec<Note>> {
        self.get_paginated(&format!(
            "/projects/{project_id}/merge_requests/{mr_iid}/notes"
        ))
    }
}

struct ActivityReport {
    weeks: HashMap<String, Vec<ActivityEntry>>,
}

struct ActivityEntry {
    date: DateTime<Utc>,
    project_name: String,
    project_url: String,
    action: String,
    details: String,
    diffs: Vec<DiffEntry>,
}

struct DiffEntry {
    file_path: String,
    change_type: String,
    diff_content: String,
}

struct DateRange {
    start: NaiveDate,
    end: NaiveDate,
}

impl DateRange {
    fn contains(&self, date: NaiveDate) -> bool {
        date >= self.start && date <= self.end
    }

    fn since_timestamp(&self) -> String {
        self.start.format("%Y-%m-%dT00:00:00Z").to_string()
    }

    fn until_timestamp(&self) -> String {
        (self.end + Duration::days(1))
            .format("%Y-%m-%dT00:00:00Z")
            .to_string()
    }
}

impl ActivityReport {
    fn new() -> Self {
        Self {
            weeks: HashMap::new(),
        }
    }

    fn add_entry(&mut self, entry: ActivityEntry) {
        let week_start = get_week_start(&entry.date);
        let week_key = week_start.format("%Y-%m-%d").to_string();
        self.weeks.entry(week_key).or_default().push(entry);
    }

    fn to_markdown(&self) -> String {
        let mut md = String::new();
        writeln!(md, "# GitLab Activity Report\n").unwrap();
        writeln!(
            md,
            "Generated on: {}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        )
        .unwrap();

        let mut weeks: Vec<_> = self.weeks.keys().collect();
        weeks.sort_by(|left, right| right.cmp(left));

        for week_key in weeks {
            let entries = self.weeks.get(week_key).unwrap();
            let week_start = NaiveDate::parse_from_str(week_key, "%Y-%m-%d").unwrap();
            let week_end = week_start + Duration::days(6);

            writeln!(
                md,
                "## Week: {} to {}\n",
                week_start.format("%B %d, %Y"),
                week_end.format("%B %d, %Y")
            )
            .unwrap();

            let mut sorted_entries: Vec<_> = entries.iter().collect();
            sorted_entries.sort_by(|left, right| right.date.cmp(&left.date));

            let mut current_date = String::new();
            for entry in sorted_entries {
                let entry_date = entry.date.format("%Y-%m-%d").to_string();
                if entry_date != current_date {
                    current_date = entry_date;
                    writeln!(md, "### {}\n", entry.date.format("%A, %B %d, %Y")).unwrap();
                }

                writeln!(
                    md,
                    "#### {} - [{}]({})\n",
                    entry.action, entry.project_name, entry.project_url
                )
                .unwrap();
                writeln!(md, "{}\n", entry.details).unwrap();

                if !entry.diffs.is_empty() {
                    md.push_str("<details>\n<summary>Code Changes</summary>\n\n");
                    for diff in &entry.diffs {
                        writeln!(md, "**{}** ({})\n", diff.file_path, diff.change_type).unwrap();
                        md.push_str("```diff\n");
                        md.push_str(&truncate_diff(&diff.diff_content, DIFF_LINE_LIMIT));
                        md.push_str("\n```\n\n");
                    }
                    md.push_str("</details>\n\n");
                }

                md.push_str("---\n\n");
            }
        }

        md
    }
}

fn parse_month(month_str: &str) -> Result<(NaiveDate, NaiveDate)> {
    let parts: Vec<&str> = month_str.split('/').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid month format. Use MM/YYYY (e.g., 01/2025)");
    }

    let month: u32 = parts[0].parse().context("Invalid month number")?;
    let year: i32 = parts[1].parse().context("Invalid year")?;

    if !(1..=12).contains(&month) {
        anyhow::bail!("Month must be between 1 and 12");
    }

    let start_date = NaiveDate::from_ymd_opt(year, month, 1).context("Invalid date")?;
    let next_month = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .context("Invalid date")?;

    Ok((start_date, next_month - Duration::days(1)))
}

fn get_week_start(date: &DateTime<Utc>) -> NaiveDate {
    let naive = date.date_naive();
    let weekday = naive.weekday().num_days_from_monday();
    naive - Duration::days(weekday as i64)
}

fn truncate_diff(diff: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = diff.lines().collect();
    if lines.len() <= max_lines {
        diff.to_string()
    } else {
        let truncated = lines.into_iter().take(max_lines).collect::<Vec<_>>();
        format!(
            "{}\n... ({} more lines truncated)",
            truncated.join("\n"),
            diff.lines().count() - max_lines
        )
    }
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        text.to_string()
    } else {
        let truncated = text.chars().take(max_chars).collect::<String>();
        format!("{truncated}...")
    }
}

fn resolve_date_range(args: &Args) -> Result<DateRange> {
    if let Some(month_str) = &args.month {
        let (start, end) = parse_month(month_str)?;
        println!(
            "Fetching activity for {} {} ({} to {})...",
            start.format("%B"),
            start.year(),
            start.format("%Y-%m-%d"),
            end.format("%Y-%m-%d")
        );
        Ok(DateRange { start, end })
    } else {
        let end = Utc::now().date_naive();
        let start = end - Duration::days(args.days as i64);
        println!("Fetching activity from {} to {}...", start, end);
        Ok(DateRange { start, end })
    }
}

fn build_commit_details(commit: &Commit, commit_url: &str) -> String {
    let body = commit
        .message
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed_body = body.trim();

    if trimmed_body.is_empty() {
        format!("**{}**\n\n[View Commit]({commit_url})", commit.title)
    } else {
        format!(
            "**{}**\n\n{}\n\n[View Commit]({commit_url})",
            commit.title, trimmed_body
        )
    }
}

fn build_issue_details(issue: &Issue) -> String {
    let description = issue.description.as_deref().unwrap_or("").trim();

    if description.is_empty() {
        format!("**{}**\n\n[View Issue]({})", issue.title, issue.web_url)
    } else {
        format!(
            "**{}**\n\n{}\n\n[View Issue]({})",
            issue.title, description, issue.web_url
        )
    }
}

fn build_mr_details(mr: &MergeRequest) -> String {
    let description = mr.description.as_deref().unwrap_or("").trim();

    if description.is_empty() {
        format!(
            "**{}**\n\n`{}` -> `{}`\n\n[View MR]({})",
            mr.title, mr.source_branch, mr.target_branch, mr.web_url
        )
    } else {
        format!(
            "**{}**\n\n{}\n\n`{}` -> `{}`\n\n[View MR]({})",
            mr.title, description, mr.source_branch, mr.target_branch, mr.web_url
        )
    }
}

fn build_note_details(title: &str, note_body: &str, url: &str) -> String {
    format!(
        "**{}**\n\n{}\n\n[Open Thread]({url})",
        title,
        truncate_text(note_body, NOTE_PREVIEW_LIMIT)
    )
}

fn collect_project_activity(
    client: &GitLabClient,
    user: &User,
    project: &Project,
    date_range: &DateRange,
    since_date: &str,
    until_date: &str,
    report: &mut ActivityReport,
) -> (usize, usize, usize, bool) {
    let mut commit_count = 0;
    let mut issue_count = 0;
    let mut mr_count = 0;
    let mut project_activity = false;

    if let Ok(commits) =
        client.get_project_commits_by_author(project.id, &user.username, since_date, until_date)
    {
        let valid_commits: Vec<_> = commits
            .into_iter()
            .filter(|commit| date_range.contains(commit.authored_date.date_naive()))
            .collect();

        if !valid_commits.is_empty() {
            project_activity = true;
            commit_count += valid_commits.len();

            for commit in valid_commits {
                let mut diffs = Vec::new();
                if let Ok(diff_data) = client.get_commit_diff(project.id, &commit.id) {
                    for diff in diff_data {
                        let change_type = if diff.new_file {
                            "added"
                        } else if diff.deleted_file {
                            "deleted"
                        } else if diff.renamed_file {
                            "renamed"
                        } else {
                            "modified"
                        };

                        diffs.push(DiffEntry {
                            file_path: diff.new_path,
                            change_type: change_type.to_string(),
                            diff_content: diff.diff,
                        });
                    }
                }

                let commit_url = commit
                    .web_url
                    .clone()
                    .unwrap_or_else(|| format!("{}/commit/{}", project.web_url, commit.id));

                report.add_entry(ActivityEntry {
                    date: commit.authored_date,
                    project_name: project.name.clone(),
                    project_url: project.web_url.clone(),
                    action: format!("🚀 Commit `{}`", commit.short_id),
                    details: build_commit_details(&commit, &commit_url),
                    diffs,
                });
            }
        }
    }

    if let Ok(issues) =
        client.get_project_issues_by_author(project.id, &user.username, since_date, until_date)
    {
        let valid_issues: Vec<_> = issues
            .into_iter()
            .filter(|issue| date_range.contains(issue.created_at.date_naive()))
            .collect();

        if !valid_issues.is_empty() {
            project_activity = true;
            issue_count += valid_issues.len();

            for issue in valid_issues {
                let state_icon = match issue.state.as_str() {
                    "opened" => "🟢",
                    "closed" => "🔴",
                    _ => "📋",
                };

                report.add_entry(ActivityEntry {
                    date: issue.created_at,
                    project_name: project.name.clone(),
                    project_url: project.web_url.clone(),
                    action: format!("{state_icon} Issue #{}", issue.iid),
                    details: build_issue_details(&issue),
                    diffs: vec![],
                });
            }
        }
    }

    if let Ok(mrs) = client.get_project_merge_requests_by_author(
        project.id,
        &user.username,
        since_date,
        until_date,
    ) {
        let valid_mrs: Vec<_> = mrs
            .into_iter()
            .filter(|mr| date_range.contains(mr.created_at.date_naive()))
            .collect();

        if !valid_mrs.is_empty() {
            project_activity = true;
            mr_count += valid_mrs.len();

            for mr in valid_mrs {
                let state_icon = match mr.state.as_str() {
                    "opened" => "🔀",
                    "merged" => "✅",
                    "closed" => "❌",
                    _ => "📝",
                };

                report.add_entry(ActivityEntry {
                    date: mr.created_at,
                    project_name: project.name.clone(),
                    project_url: project.web_url.clone(),
                    action: format!("{state_icon} MR !{}", mr.iid),
                    details: build_mr_details(&mr),
                    diffs: vec![],
                });
            }
        }
    }

    (commit_count, issue_count, mr_count, project_activity)
}

fn collect_comment_activity(
    client: &GitLabClient,
    user: &User,
    projects: &[Project],
    date_range: &DateRange,
    since_date: &str,
    until_date: &str,
    report: &mut ActivityReport,
) -> usize {
    let mut total_comments = 0;

    for project in projects {
        if let Ok(issues) =
            client.get_project_issues_updated_in_range(project.id, since_date, until_date)
        {
            for issue in issues {
                if let Ok(notes) = client.get_issue_notes(project.id, issue.iid) {
                    for note in notes {
                        if note.system
                            || note.author.username != user.username
                            || !date_range.contains(note.created_at.date_naive())
                        {
                            continue;
                        }

                        total_comments += 1;
                        report.add_entry(ActivityEntry {
                            date: note.created_at,
                            project_name: project.name.clone(),
                            project_url: project.web_url.clone(),
                            action: format!("💬 Comment on Issue #{}", issue.iid),
                            details: build_note_details(&issue.title, &note.body, &issue.web_url),
                            diffs: vec![],
                        });
                    }
                }
            }
        }

        if let Ok(mrs) = client.get_project_mrs_updated_in_range(project.id, since_date, until_date)
        {
            for mr in mrs {
                if let Ok(notes) = client.get_mr_notes(project.id, mr.iid) {
                    for note in notes {
                        if note.system
                            || note.author.username != user.username
                            || !date_range.contains(note.created_at.date_naive())
                        {
                            continue;
                        }

                        total_comments += 1;
                        report.add_entry(ActivityEntry {
                            date: note.created_at,
                            project_name: project.name.clone(),
                            project_url: project.web_url.clone(),
                            action: format!("💬 Comment on MR !{}", mr.iid),
                            details: build_note_details(&mr.title, &note.body, &mr.web_url),
                            diffs: vec![],
                        });
                    }
                }
            }
        }
    }

    total_comments
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Connecting to GitLab at {}...", args.url);
    let client = GitLabClient::new(&args.url, &args.token);

    let user = client.get_current_user()?;
    println!("Authenticated as: {} (@{})", user.name, user.username);

    let date_range = resolve_date_range(&args)?;
    let since_date = date_range.since_timestamp();
    let until_date = date_range.until_timestamp();

    println!("Fetching projects...");
    let projects = client.get_user_projects()?;
    println!("Found {} projects", projects.len());

    let mut report = ActivityReport::new();
    let mut total_commits = 0;
    let mut total_issues = 0;
    let mut total_mrs = 0;

    for project in &projects {
        print!("Checking {}... ", project.name);
        std::io::stdout().flush()?;

        let (project_commits, project_issues, project_mrs, project_activity) =
            collect_project_activity(
                &client,
                &user,
                project,
                &date_range,
                &since_date,
                &until_date,
                &mut report,
            );

        total_commits += project_commits;
        total_issues += project_issues;
        total_mrs += project_mrs;

        if project_activity {
            println!("found activity");
        } else {
            println!("no activity");
        }
    }

    println!("\nFetching comments...");
    let total_comments = collect_comment_activity(
        &client,
        &user,
        &projects,
        &date_range,
        &since_date,
        &until_date,
        &mut report,
    );

    println!(
        "Found {} commits, {} issues, {} merge requests, {} comments",
        total_commits, total_issues, total_mrs, total_comments
    );

    println!("\nGenerating report...");

    let markdown = report.to_markdown();
    let mut file = File::create(&args.output)?;
    file.write_all(markdown.as_bytes())?;

    println!("Report saved to: {}", args.output);

    Ok(())
}
