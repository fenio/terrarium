//! Interactive shortcut commands. Parse arguments before interpolation so
//! resource/context names remain data, never shell syntax or extra arguments.

use anyhow::{Context, bail};

use crate::config::Shortcut;

/// Give an interactive child its own foreground process group. Ctrl-C and
/// Ctrl-\ then go to the child/session, not to the suspended Terrarium process.
#[cfg(unix)]
pub(crate) fn run_interactive(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    use std::os::unix::process::CommandExt;

    // SAFETY: tcgetpgrp only reads the terminal's foreground process group.
    let original_group = unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) };
    if original_group < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the pre-exec closure only calls async-signal-safe libc functions.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            set_foreground_group(libc::getpgrp())
        });
    }
    let result = command.status();
    // The child may fail after taking foreground ownership (e.g. exec ENOENT),
    // so restore it for both successful waits and spawn failures.
    set_foreground_group(original_group)?;
    result
}

#[cfg(not(unix))]
pub(crate) fn run_interactive(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    command.status()
}

#[cfg(unix)]
fn set_foreground_group(group: libc::pid_t) -> std::io::Result<()> {
    // A background process calling tcsetpgrp normally receives SIGTTOU. Block
    // it on this thread only during the handoff, preserving all other signals
    // and the caller's signal mask (including in the child before exec).
    unsafe {
        let mut blocked: libc::sigset_t = std::mem::zeroed();
        let mut previous: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut blocked);
        libc::sigaddset(&mut blocked, libc::SIGTTOU);
        let error = libc::sigprocmask(libc::SIG_BLOCK, &blocked, &mut previous);
        if error < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let result = if libc::tcsetpgrp(libc::STDIN_FILENO, group) < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        };
        if libc::sigprocmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut()) < 0 {
            return Err(std::io::Error::last_os_error());
        }
        result
    }
}

pub(crate) fn command(
    shortcut: &Shortcut,
    namespace: &str,
    full_name: &str,
    context: &str,
) -> anyhow::Result<std::process::Command> {
    if shortcut.url.is_some() || !shortcut.children.is_empty() {
        bail!("a launcher cannot also define url or children");
    }
    let template = shortcut.launcher.as_deref().context("missing launcher")?;
    let words = shell_words::split(template).context("invalid launcher quoting")?;
    let (program, args) = words.split_first().context("launcher is empty")?;
    if program.is_empty() || program.contains(['{', '}']) {
        bail!("launcher executable must be a non-empty literal, not a placeholder");
    }
    let name = match shortcut.name_strip_prefix.as_deref() {
        Some(prefix) => full_name.strip_prefix(prefix).unwrap_or(full_name),
        None => full_name,
    };
    if name.is_empty() {
        bail!("name_strip_prefix would leave an empty resource name");
    }
    let mut command = std::process::Command::new(program);
    for arg in args {
        command.arg(expand(arg, name, full_name, namespace, context)?);
    }
    Ok(command)
}

fn expand(
    template: &str,
    name: &str,
    full_name: &str,
    namespace: &str,
    context: &str,
) -> anyhow::Result<String> {
    let mut result = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let end = tail
            .find('}')
            .context("unterminated launcher placeholder")?;
        result.push_str(match &tail[..end] {
            "name" => name,
            "full_name" => full_name,
            "namespace" => namespace,
            "context" => context,
            unknown => bail!("unsupported launcher placeholder {{{unknown}}}"),
        });
        rest = &tail[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shortcut(template: &str, prefix: Option<&str>) -> Shortcut {
        Shortcut {
            key: 'J',
            label: "Cluster shell".into(),
            description: None,
            group: None,
            url: None,
            launcher: Some(template.into()),
            name_strip_prefix: prefix.map(str::to_owned),
            when: None,
            children: Vec::new(),
        }
    }

    fn args(command: &std::process::Command) -> Vec<&str> {
        command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect()
    }

    #[test]
    fn strips_configured_prefix_once() {
        let cmd = command(
            &shortcut("cluster-switcher {name} {full_name}", Some("cluster-")),
            "ns",
            "cluster-us-ord-tsdb-aclp01-prod",
            "ctx",
        )
        .unwrap();
        assert_eq!(cmd.get_program(), "cluster-switcher");
        assert_eq!(
            args(&cmd),
            ["us-ord-tsdb-aclp01-prod", "cluster-us-ord-tsdb-aclp01-prod"]
        );
        let cmd = command(
            &shortcut("tool {name}", Some("cluster-")),
            "ns",
            "cluster-cluster-demo",
            "ctx",
        )
        .unwrap();
        assert_eq!(args(&cmd), ["cluster-demo"]);
    }

    #[test]
    fn absent_or_nonmatching_prefix_preserves_name() {
        for prefix in [None, Some("other-")] {
            let cmd = command(
                &shortcut("tool {name}", prefix),
                "ns",
                "cluster-demo",
                "ctx",
            )
            .unwrap();
            assert_eq!(args(&cmd), ["cluster-demo"]);
        }
    }

    #[test]
    fn substitutions_are_single_arguments_and_are_not_recursive() {
        let cmd = command(
            &shortcut("tool --context={context} '{namespace}' {name}", None),
            "namespace with spaces",
            "cluster-demo",
            "ctx'; $(touch /tmp/not-executed) {name}",
        )
        .unwrap();
        assert_eq!(
            args(&cmd),
            [
                "--context=ctx'; $(touch /tmp/not-executed) {name}",
                "namespace with spaces",
                "cluster-demo",
            ]
        );
    }

    #[test]
    fn rejects_invalid_templates_and_ambiguous_shortcuts() {
        for template in [
            "",
            "''",
            "{name}",
            "tool '",
            "tool {output.cluster}",
            "tool {name",
        ] {
            assert!(command(&shortcut(template, None), "ns", "demo", "ctx").is_err());
        }
        assert!(command(&shortcut("tool {name}", Some("demo")), "ns", "demo", "ctx").is_err());
        let mut s = shortcut("tool {name}", None);
        s.url = Some("https://example.com".into());
        assert!(command(&s, "ns", "demo", "ctx").is_err());
        s.url = None;
        s.children.push(shortcut("tool", None));
        assert!(command(&s, "ns", "demo", "ctx").is_err());
    }

    #[test]
    fn launcher_config_deserializes() {
        let config: crate::config::Config = toml::from_str(
            r#"
            [[shortcuts]]
            key = "J"
            label = "Cluster shell"
            launcher = "cluster-switcher {name}"
            name_strip_prefix = "cluster-"
            [shortcuts.when]
            name = "^cluster-"
            "#,
        )
        .unwrap();
        assert_eq!(
            config.shortcuts[0].name_strip_prefix.as_deref(),
            Some("cluster-")
        );
        assert!(config.shortcuts[0].url.is_none());
    }

    #[test]
    fn example_config_accepts_launchers_and_existing_urls() {
        let config: crate::config::Config =
            toml::from_str(include_str!("../examples/shortcuts.toml")).unwrap();
        assert!(config.shortcuts.iter().any(|s| s.launcher.is_some()));
        assert!(config.shortcuts.iter().any(|s| s.url.is_some()));
    }

    #[cfg(unix)]
    #[test]
    fn process_receives_literal_arguments() {
        let malicious = "context with spaces; $(echo injected) 'quoted' {name}";
        let mut cmd = command(
            &shortcut(
                r#"sh -c 'printf "%s\n" "$1" "$2"' -- {context} {name}"#,
                Some("cluster-"),
            ),
            "ns",
            "cluster-demo",
            malicious,
        )
        .unwrap();
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{malicious}\ndemo\n")
        );
    }
}
