//! File tools. `edit_file` uses an ordered replacer ladder (opencode `tool/edit.ts`
//! lineage): exact → line-trimmed → whitespace-normalized. Ambiguous matches are refused
//! rather than guessed; the model is told how many matches and asked for more context.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Risk, Tool, ToolContext, ToolOutput};

fn resolve(ctx: &ToolContext, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.cwd.join(path)
    }
}

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn description(&self) -> String {
        "Read a text file. Optional 1-based line range. Lines are prefixed with numbers.".into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{
            "path":{"type":"string"},
            "start_line":{"type":"integer"},
            "end_line":{"type":"integer"}
        },"required":["path"]})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::ReadOnly
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let p = resolve(
            ctx,
            input["path"]
                .as_str()
                .ok_or_else(|| anyhow!("path required"))?,
        );
        let text = tokio::fs::read_to_string(&p)
            .await
            .map_err(|e| anyhow!("{}: {e}", p.display()))?;
        let start = input["start_line"].as_u64().unwrap_or(1).max(1) as usize;
        let end = input["end_line"]
            .as_u64()
            .map(|v| v as usize)
            .unwrap_or(usize::MAX);
        let out: Vec<String> = text
            .lines()
            .enumerate()
            .filter(|(i, _)| *i + 1 >= start && *i < end)
            .map(|(i, l)| format!("{:>5}\t{l}", i + 1))
            .collect();
        Ok(ToolOutput::ok(out.join("\n")))
    }
}

pub struct WriteFile;

#[async_trait]
impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn description(&self) -> String {
        "Create or overwrite a file with the given content. Creates parent directories.".into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]})
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let p = resolve(
            ctx,
            input["path"]
                .as_str()
                .ok_or_else(|| anyhow!("path required"))?,
        );
        let content = input["content"].as_str().unwrap_or("");
        if let Some(parent) = p.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let existed = p.exists();
        tokio::fs::write(&p, content).await?;
        Ok(ToolOutput::ok(format!(
            "{} {} ({} bytes)",
            if existed { "updated" } else { "created" },
            p.display(),
            content.len()
        ))
        .with_data(
            json!({"path": p.display().to_string(), "kind": if existed {"update"} else {"add"}}),
        ))
    }
}

pub struct ListDir;

#[async_trait]
impl Tool for ListDir {
    fn name(&self) -> &'static str {
        "list_dir"
    }
    fn description(&self) -> String {
        "List a directory (non-recursive). Directories end with '/'.".into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":[]})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::ReadOnly
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let p = resolve(ctx, input["path"].as_str().unwrap_or("."));
        let mut rd = tokio::fs::read_dir(&p)
            .await
            .map_err(|e| anyhow!("{}: {e}", p.display()))?;
        let mut names = vec![];
        while let Some(e) = rd.next_entry().await? {
            let n = e.file_name().to_string_lossy().to_string();
            names.push(
                if e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                    format!("{n}/")
                } else {
                    n
                },
            );
        }
        names.sort();
        Ok(ToolOutput::ok(names.join("\n")))
    }
}

pub struct EditFile;

/// Replacer ladder. Returns (new_text, strategy) or Err with a diagnostic.
pub fn apply_edit(
    text: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<(String, &'static str)> {
    if old.is_empty() {
        return Err(anyhow!("old_string must not be empty"));
    }
    // 1. exact
    let n = text.matches(old).count();
    if n == 1 || (n > 1 && replace_all) {
        return Ok((text.replace(old, new), "exact"));
    }
    if n > 1 {
        return Err(anyhow!(
            "old_string matched {n} times; add surrounding context or set replace_all"
        ));
    }
    // 2. line-trimmed: compare lines with trailing/leading whitespace trimmed
    let old_lines: Vec<&str> = old.lines().map(str::trim).collect();
    let text_lines: Vec<&str> = text.lines().collect();
    let mut hits = vec![];
    if !old_lines.is_empty() && text_lines.len() >= old_lines.len() {
        for i in 0..=(text_lines.len() - old_lines.len()) {
            if text_lines[i..i + old_lines.len()]
                .iter()
                .map(|l| l.trim())
                .eq(old_lines.iter().copied())
            {
                hits.push(i);
            }
        }
    }
    if hits.len() == 1 || (hits.len() > 1 && replace_all) {
        let mut out: Vec<String> = text_lines.iter().map(|s| s.to_string()).collect();
        for &i in hits.iter().rev() {
            // preserve the indentation of the first matched line
            let indent: String = text_lines[i]
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            let repl: Vec<String> = new
                .lines()
                .map(|l| format!("{indent}{}", l.trim_start()))
                .collect();
            out.splice(i..i + old_lines.len(), repl);
        }
        let mut s = out.join("\n");
        if text.ends_with('\n') {
            s.push('\n');
        }
        return Ok((s, "line_trimmed"));
    }
    if hits.len() > 1 {
        return Err(anyhow!(
            "old_string matched {} times after whitespace normalization; add context",
            hits.len()
        ));
    }
    // 3. whitespace-normalized (collapse runs of whitespace)
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let nt = norm(text);
    let no = norm(old);
    if !no.is_empty() && nt.matches(&no).count() == 1 {
        // Locate by scanning windows of lines whose normalized form equals `no`.
        for win in 1..=old.lines().count().max(1) + 2 {
            for i in 0..text_lines.len().saturating_sub(win - 1) {
                let seg = text_lines[i..i + win].join("\n");
                if norm(&seg) == no {
                    let mut out: Vec<String> = text_lines.iter().map(|s| s.to_string()).collect();
                    let indent: String = text_lines[i]
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .collect();
                    let repl: Vec<String> = new
                        .lines()
                        .map(|l| format!("{indent}{}", l.trim_start()))
                        .collect();
                    out.splice(i..i + win, repl);
                    let mut s = out.join("\n");
                    if text.ends_with('\n') {
                        s.push('\n');
                    }
                    return Ok((s, "whitespace_normalized"));
                }
            }
        }
    }
    Err(anyhow!("old_string not found (tried exact, line-trimmed, whitespace-normalized). Re-read the file and copy the text verbatim."))
}

#[async_trait]
impl Tool for EditFile {
    fn name(&self) -> &'static str {
        "edit_file"
    }
    fn description(&self) -> String {
        "Replace `old_string` with `new_string` in a file. old_string must match exactly once \
         (whitespace-tolerant fallbacks are tried); use replace_all for multiple. Read the file first."
            .into()
    }
    fn schema(&self) -> Value {
        json!({"type":"object","properties":{
            "path":{"type":"string"},
            "old_string":{"type":"string"},
            "new_string":{"type":"string"},
            "replace_all":{"type":"boolean"}
        },"required":["path","old_string","new_string"]})
    }
    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let p = resolve(
            ctx,
            input["path"]
                .as_str()
                .ok_or_else(|| anyhow!("path required"))?,
        );
        let text = tokio::fs::read_to_string(&p)
            .await
            .map_err(|e| anyhow!("{}: {e}", p.display()))?;
        let old = input["old_string"].as_str().unwrap_or("");
        let new = input["new_string"].as_str().unwrap_or("");
        let all = input["replace_all"].as_bool().unwrap_or(false);
        match apply_edit(&text, old, new, all) {
            Ok((updated, strategy)) => {
                tokio::fs::write(&p, &updated).await?;
                let diff = similar::TextDiff::from_lines(&text, &updated)
                    .unified_diff()
                    .context_radius(2)
                    .header(&p.display().to_string(), &p.display().to_string())
                    .to_string();
                Ok(ToolOutput::ok(format!("edited {} via {strategy}\n{diff}", p.display()))
                    .with_data(json!({"path": p.display().to_string(), "kind": "update", "strategy": strategy})))
            }
            Err(e) => Ok(ToolOutput::err(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::apply_edit;

    #[test]
    fn ladder() {
        let t = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
        let (s, how) = apply_edit(t, "    let x = 1;", "    let x = 10;", false).unwrap();
        assert_eq!(how, "exact");
        assert!(s.contains("let x = 10;"));
        let (s, how) = apply_edit(t, "\tlet y = 2;", "let y = 20;", false).unwrap();
        assert_eq!(how, "line_trimmed");
        assert!(s.contains("    let y = 20;"));
        let (_, how) = apply_edit(t, "let  x   = 1;\n let y = 2;", "let z = 3;", false).unwrap();
        assert_eq!(how, "whitespace_normalized");
        assert!(apply_edit(t, "let", "x", false).is_err());
    }
}
