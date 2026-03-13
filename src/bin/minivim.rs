use anyhow::Result;

fn main() -> Result<()> {
    minimacs::run(minimacs::KeybindingMode::Vim)
}
