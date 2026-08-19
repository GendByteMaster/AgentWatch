pub trait AgentProvider {
    fn name(&self) -> &'static str;
    fn executable(&self) -> &'static str;
    fn build_args(&self, user_args: &[String]) -> Vec<String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CodexProvider;

impl AgentProvider for CodexProvider {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn executable(&self) -> &'static str {
        "codex"
    }

    fn build_args(&self, user_args: &[String]) -> Vec<String> {
        let mut args = Vec::with_capacity(user_args.len() + 1);
        args.push("exec".to_owned());
        args.extend(user_args.iter().cloned());
        args
    }
}
