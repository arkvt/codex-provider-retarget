use std::collections::{HashMap, HashSet};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use super::types::RunningProcess;

struct Candidate {
    pid: u32,
    name: String,
    path: Option<String>,
    parent: Option<u32>,
}

/// Every Codex CLI / Codex App process that would fight us for the database or
/// the session files.
///
/// The OS is queried directly so detection does not depend on the output format
/// or localization of external commands.
pub fn find_running_codex_processes() -> Vec<RunningProcess> {
    let mut system = System::new();
    // Only the command line and executable path are needed; they identify Codex
    // inside a generic host process such as `node.exe` or `ChatGPT.exe`.
    //
    // `ProcessRefreshKind::everything()` must NOT be used here: it also resolves
    // user accounts and reads environment blocks, which on a network-joined
    // Windows machine can stall for minutes per process.
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always),
    );

    let self_pid = std::process::id();
    let mut candidates = Vec::new();

    for (pid, process) in system.processes() {
        let pid = pid.as_u32();
        if pid == self_pid {
            continue;
        }

        let name = process.name().to_string_lossy().to_string();
        let path = process
            .exe()
            .map(|value| value.to_string_lossy().to_string())
            .filter(|value| !value.is_empty());
        let command_line = process
            .cmd()
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");

        if !is_codex_process(&name, path.as_deref(), &command_line) {
            continue;
        }

        candidates.push(Candidate {
            pid,
            name,
            path: path.or_else(|| {
                (!command_line.is_empty()).then_some(command_line.clone())
            }),
            parent: process.parent().map(|value| value.as_u32()),
        });
    }

    let parents: HashMap<u32, Option<u32>> = candidates
        .iter()
        .map(|candidate| (candidate.pid, candidate.parent))
        .collect();
    let candidate_pids: HashSet<u32> = candidates.iter().map(|candidate| candidate.pid).collect();

    let mut found: Vec<RunningProcess> = candidates
        .iter()
        .filter(|candidate| !has_candidate_ancestor(candidate.pid, &parents, &candidate_pids))
        .map(|candidate| RunningProcess {
            pid: candidate.pid,
            name: candidate.name.clone(),
            path: candidate.path.clone(),
        })
        .collect();

    found.sort_by_key(|process| process.pid);
    found
}

/// Lowercase and normalise separators so the same substring tests work on
/// Windows paths regardless of how the OS reports them.
fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('/', "\\")
}

fn is_codex_process(name: &str, exe: Option<&str>, command_line: &str) -> bool {
    let normalized_name = name.trim().to_ascii_lowercase();
    if normalized_name == "codex" || normalized_name == "codex.exe" {
        return true;
    }

    let details = format!(
        "{}\n{}",
        normalize(exe.unwrap_or_default()),
        normalize(command_line)
    );

    // The Codex desktop app ships inside a versioned `OpenAI.Codex_*` folder
    // and runs as `ChatGPT.exe`; the plain ChatGPT app must not be matched.
    if normalized_name == "chatgpt" || normalized_name == "chatgpt.exe" {
        return details.contains("\\openai.codex_");
    }

    // `npm install -g @openai/codex` runs the CLI under `node.exe`.
    if normalized_name == "node" || normalized_name == "node.exe" {
        return details.contains("\\@openai\\codex\\bin\\codex.js");
    }

    false
}

/// A wrapper such as `ChatGPT.exe` plus its `codex.exe` child would otherwise be
/// listed twice; only the outermost process is reported.
fn has_candidate_ancestor(
    pid: u32,
    parents: &HashMap<u32, Option<u32>>,
    candidate_pids: &HashSet<u32>,
) -> bool {
    let mut visited = HashSet::from([pid]);
    let mut current = parents.get(&pid).copied().flatten();

    while let Some(parent) = current {
        if !visited.insert(parent) {
            return false;
        }
        if candidate_pids.contains(&parent) {
            return true;
        }
        current = parents.get(&parent).copied().flatten();
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_codex_binaries_by_name() {
        assert!(is_codex_process("codex.exe", None, ""));
        assert!(is_codex_process("Codex", None, ""));
        assert!(!is_codex_process("chatgpt.exe", Some(r"C:\Program Files\ChatGPT\ChatGPT.exe"), ""));
    }

    #[test]
    fn recognises_the_codex_desktop_app_hosted_in_chatgpt() {
        let exe = Some(r"C:\Users\me\AppData\Local\Programs\OpenAI.Codex_1.2.3\ChatGPT.exe");
        assert!(is_codex_process("ChatGPT.exe", exe, ""));

        let via_cmd = r#""C:\Users\me\AppData\Local\Programs\OpenAI.Codex_1.2.3\x64\ChatGPT.exe""#;
        assert!(is_codex_process("ChatGPT.exe", None, via_cmd));
    }

    #[test]
    fn recognises_the_npm_cli_hosted_in_node() {
        let cmd = r#"node C:\Users\me\AppData\Roaming\npm\node_modules\@openai\codex\bin\codex.js"#;
        assert!(is_codex_process("node.exe", None, cmd));

        // An ordinary node script is not Codex.
        assert!(!is_codex_process("node.exe", None, "node server.js"));
    }

    #[test]
    fn forwarded_slashes_are_normalised() {
        let cmd = "node /usr/local/lib/node_modules/@openai/codex/bin/codex.js";
        assert!(is_codex_process("node", None, cmd));
    }

    #[test]
    fn unrelated_processes_are_ignored() {
        assert!(!is_codex_process("explorer.exe", Some(r"C:\Windows\explorer.exe"), ""));
        assert!(!is_codex_process("python.exe", None, "python codex_helper.py"));
    }

    #[test]
    fn nested_candidates_collapse_to_the_outermost_process() {
        // 10 (ChatGPT.exe) -> 20 (codex.exe) -> 30 (node.exe)
        let parents = HashMap::from([(10, None), (20, Some(10)), (30, Some(20))]);
        let candidates = HashSet::from([10, 20, 30]);

        assert!(!has_candidate_ancestor(10, &parents, &candidates));
        assert!(has_candidate_ancestor(20, &parents, &candidates));
        assert!(has_candidate_ancestor(30, &parents, &candidates));
    }

    #[test]
    fn unrelated_ancestors_do_not_collapse_a_candidate() {
        let parents = HashMap::from([(10, Some(1)), (99, None)]);
        let candidates = HashSet::from([10, 99]);
        assert!(!has_candidate_ancestor(10, &parents, &candidates));
    }

    #[test]
    fn cyclic_parent_links_terminate() {
        let parents = HashMap::from([(1, Some(2)), (2, Some(1))]);
        let candidates = HashSet::from([1, 2]);
        assert!(has_candidate_ancestor(1, &parents, &candidates));
    }

    #[test]
    fn the_real_probe_runs_and_never_reports_this_test_process() {
        let found = find_running_codex_processes();
        assert!(
            !found.iter().any(|process| process.pid == std::process::id()),
            "the tool must not report itself as a Codex process"
        );
    }
}
