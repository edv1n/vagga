use std::io::{stdout, stderr, Write};
use std::rc::Rc;

use argparse::{ArgumentParser, StoreTrue, StoreConst};

use config::Config;
use launcher::sphinx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Zsh,
    Sphinx,
}


pub fn print_list(config: &Config, mut args: Vec<String>)
    -> Result<i32, String>
{
    let mut all = false;
    let mut builtin = false;
    let mut hidden = false;
    let mut containers = false;
    let mut format = Format::Text;
    let mut verbose = false;
    {
        args.insert(0, String::from("vagga _list"));
        let mut ap = ArgumentParser::new();
        ap.refer(&mut containers)
            .add_option(&["--containers"], StoreTrue,
                "Show containers instead of commands");
        ap.refer(&mut all)
            .add_option(&["-A", "--all"], StoreTrue,
                "Show all commands");
        ap.refer(&mut builtin)
            .add_option(&["--builtin"], StoreTrue,
                "Show built-in commands (starting with underscore)");
        ap.refer(&mut hidden)
            .add_option(&["--hidden"], StoreTrue,
                "Show hidden commands");
        ap.refer(&mut format)
            .add_option(&["--zsh"], StoreConst(Format::Zsh),
                "Use zsh completion compatible format")
            .add_option(&["--sphinx"], StoreConst(Format::Sphinx),
                "Print sphinx-friendly restructured text (experimental)");
        ap.refer(&mut verbose)
            .add_option(&["-v", "--verbose"], StoreTrue,
                "Verbose output (show source files
                 for containers and commands)");
        match ap.parse(args, &mut stdout(), &mut stderr()) {
            Ok(()) => {}
            Err(x) => return Ok(x),
        }
    }
    if containers {
        for (cname, container) in config.containers.iter() {
            println!("{}", cname);
            if let Some(ref src) = container.source {
                if verbose {
                    println!("{:19} (from {:?})", " ", &src);
                }
            }
        }
    } else if format == Format::Sphinx {
        sphinx::write_commands(&mut stdout(), config,
            hidden || all, "Vagga Commands").ok();
    } else {
        let ref builtins = Some(Rc::new("<builtins>".into()));
        let mut commands: Vec<_> = config.commands.iter()
            .map(|(name, cmd)|
                (&name[..],
                 cmd.description().unwrap_or(&"".to_string()).to_string(),
                 cmd.source()))
            .collect();
        // TODO(tailhook) fetch builtins from completion code
        commands.push(
            ("_build",
             "Build a container".to_string(),
             &builtins));
        commands.push(
            ("_run",
             "Run arbitrary command, optionally building container".to_string(),
             &builtins));
        commands.push((
            "_clean",
            "Clean containers and build artifacts".to_string(),
            &builtins));
        commands.push((
            "_list",
            "List of built-in commands".to_string(),
            &builtins));
        commands.push((
            "_base_dir",
            "Display a directory which contains vagga.yaml".to_string(),
            &builtins));
        commands.push((
            "_relative_work_dir",
            "Display a relative path from the current \
            working directory to the directory \
            containing vagga.yaml".to_string(),
            &builtins));
        commands.push((
            "_update_symlinks",
            "Updates symlinks to vagga for commands having ``symlink-name`` \
            in this project".to_string(),
            &builtins));

        let mut out = stdout();
        for (name, description, source) in commands {
            if name.starts_with("_") && !(hidden || all) {
                continue;
            }
            match format {
                Format::Zsh => {
                    let descr_line = description
                        .lines().next().unwrap_or(&"");
                    out.write_all(name.as_bytes()).ok();
                    out.write_all(":".as_bytes()).ok();
                    out.write_all(descr_line.as_bytes()).ok();
                    out.write_all(b"\n").ok();
                }
                Format::Text => {
                    out.write_all(name.as_bytes()).ok();
                    if name.len() > 19 {
                        out.write_all(b"\n                    ").ok();
                    } else {
                        for _ in name.len()..19 {
                            out.write_all(b" ").ok();
                        }
                        out.write_all(b" ").ok();
                    }
                    if description.contains("\n") {
                        for line in description.lines() {
                            out.write_all(line.as_bytes()).ok();
                            out.write_all(b"\n                    ").ok();
                        };
                    } else {
                        out.write_all(description.as_bytes()).ok();
                    }
                    out.write_all(b"\n").ok();
                    if let Some(ref src) = *source {
                        if verbose {
                            println!("{:19} (from {:?})", " ", &src);
                        }
                    }
                }
                Format::Sphinx => unreachable!(),
            }
        }
    }
    return Ok(0);
}

pub fn print_help(config: &Config)
    -> Result<i32, String>
{
    let mut err = stderr();
    writeln!(&mut err, "Available commands:").ok();
    for (k, cmd) in config.commands.iter() {
        if k.starts_with("_") {
            continue;
        }
        write!(&mut err, "    {}", k).ok();
        match cmd.description() {
            Some(ref val) => {
                if k.len() > 19 {
                    err.write_all(b"\n                        ").ok();
                } else {
                    for _ in k.len()..19 {
                        err.write_all(b" ").ok();
                    }
                    err.write_all(b" ").ok();
                }
                if val.contains("\n") {
                    for line in val.lines() {
                        err.write_all(line.as_bytes()).ok();
                        err.write_all(b"\n                        ").ok();
                    };
                } else {
                    err.write_all(val.as_bytes()).ok();
                }
            }
            None => {}
        }
        err.write_all(b"\n").ok();
    }
    Ok(127)
}
