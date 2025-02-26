use anyhow::Result;
#[allow(clippy::wildcard_imports)]
use vergen_gitcl::*;

fn main() -> Result<()> {
    let cargo = CargoBuilder::all_cargo()?;
    let rustc = RustcBuilder::all_rustc()?;

    let mut emitter = Emitter::default();
    emitter
        .add_instructions(&cargo)?
        .add_instructions(&git)?
        .add_instructions(&rustc)?;

    if let Ok(git) = GitclBuilder::default().sha(false).build() {
        emitter.add_instructions(&git)?;
    }

    emitter.emit()?;

    Ok(())
}
