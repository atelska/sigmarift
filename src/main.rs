mod control;
mod execute;
mod files;
mod model;
mod tui;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut control = control::Control::initialize()?;
    let result = tui::run(&mut control);
    control.shutdown()?;
    Ok(result?)
}
