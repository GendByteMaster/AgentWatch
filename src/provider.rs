pub trait AgentProvider {
    fn name(&self) -> &'static str;
    fn executable(&self) -> &'static str;
    fn build_args(&self, user_args: &[String]) -> Vec<String>;

    fn model(&self, _user_args: &[String]) -> Option<String> {
        None
    }
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

    fn model(&self, user_args: &[String]) -> Option<String> {
        user_args.windows(2).find_map(|pair| match pair {
            [flag, value] if flag == "--model" || flag == "-m" => Some(value.clone()),
            _ => None,
        })
    }
}
