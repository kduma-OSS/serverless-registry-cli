//! Interactive confirmation on stderr.
//!
//! Without a terminal on stdin the prompts fail instead of blocking, so pipelines must
//! pass `--yes` explicitly.

use std::io::{BufRead, IsTerminal, Write};

use anyhow::{bail, Context};

/// Asks `question` and returns whether the answer is `y` or `yes`.
pub fn confirm(question: &str) -> anyhow::Result<bool> {
    let answer = ask(&format!("{question} [y/N] "))?;
    Ok(is_yes(&answer))
}

/// Fails unless the user confirms with [`confirm`] or `yes` is set.
pub fn require(yes: bool, question: &str) -> anyhow::Result<()> {
    if yes || confirm(question)? {
        Ok(())
    } else {
        bail!("aborted")
    }
}

fn ask(prompt: &str) -> anyhow::Result<String> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!("confirmation required but stdin is not a terminal; pass --yes to confirm");
    }
    read_answer(stdin.lock(), std::io::stderr().lock(), prompt)
}

fn read_answer(
    mut input: impl BufRead,
    mut output: impl Write,
    prompt: &str,
) -> anyhow::Result<String> {
    write!(output, "{prompt}")?;
    output.flush()?;
    let mut line = String::new();
    input
        .read_line(&mut line)
        .context("failed to read the answer")?;
    Ok(line)
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_answer_after_printing_the_prompt() {
        let mut prompt = Vec::new();
        let answer = read_answer(&b"yes\n"[..], &mut prompt, "Sure? ").unwrap();
        assert_eq!(answer, "yes\n");
        assert_eq!(prompt, b"Sure? ");
    }

    #[test]
    fn only_y_and_yes_confirm() {
        for answer in ["y\n", "YES", " yes "] {
            assert!(is_yes(answer), "{answer}");
        }
        for answer in ["", "n", "no", "yess", "\n"] {
            assert!(!is_yes(answer), "{answer}");
        }
    }

    #[test]
    fn require_passes_with_yes_flag() {
        assert!(require(true, "Delete everything?").is_ok());
    }
}
