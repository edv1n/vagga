use std::io::{self, ErrorKind};
use std::fs::{File, Metadata, read_link};
use std::path::{Path, PathBuf};
use std::os::unix::fs::{PermissionsExt, MetadataExt};
use std::collections::{BTreeMap, BTreeSet, HashSet};

use libc::{uid_t, gid_t};
use quire::ast::{Ast, Tag};
use quire::validate as V;
use path_filter::{PathFilter, FilterError};

use container::root::temporary_change_root;
use file_util::shallow_copy;
use path_util::IterSelfAndParents;
use build_step::{BuildStep, VersionError, StepError, Digest, Config, Guard};
use quick_error::ResultExt;


const DEFAULT_UMASK: u32 = 0o002;

const DIR_MODE: u32 = 0o777;
const FILE_MODE: u32 = 0o666;
const EXE_FILE_MODE: u32 = 0o777;
const EXE_CHECK_MASK: u32 = 0o100;

const DEFAULT_IGNORE_REGEX: &'static str =
    r#"(^|/)\.(git|hg|svn|vagga)($|/)|~$|\.bak$|\.orig$|^#.*#$"#;

const DEFAULT_IGNORE_RULES: &'static [&'static str] = &[
    "!.git/",
    "!.hg/",
    "!.svn/",
    "!.vagga/",
    "!*.bak",
    "!*.orig",
    "!*~",
    "!#*#",
    "!.#*",
];


#[derive(RustcDecodable, Debug)]
pub struct Depends {
    pub path: PathBuf,
    pub ignore_regex: Option<String>,
    pub include_regex: Option<String>,
    pub rules: Vec<String>,
    pub no_default_rules: Option<bool>,
}

fn depends_parser(ast: Ast) -> BTreeMap<String, Ast> {
    match ast {
        Ast::Scalar(pos, _, style, value) => {
            let mut map = BTreeMap::new();
            map.insert("path".to_string(),
                Ast::Scalar(pos.clone(), Tag::NonSpecific, style, value));
            map
        },
        _ => unreachable!(),
    }
}

impl Depends {
    pub fn config() -> V::Structure<'static> {
        V::Structure::new()
            .member("path", V::Scalar::new())
            .member("ignore_regex", V::Scalar::new().optional())
            .member("include_regex", V::Scalar::new().optional())
            .member("rules", V::Sequence::new(V::Scalar::new()))
            .member("no_default_rules", V::Scalar::new().optional())
            .parser(depends_parser)
    }
}

impl BuildStep for Depends {
    fn name(&self) -> &'static str { "Depends" }
    fn hash(&self, _cfg: &Config, hash: &mut Digest)
        -> Result<(), VersionError>
    {
        let path = Path::new("/work").join(&self.path);
        let filter = create_path_filter(&self.rules, self.no_default_rules,
            &self.ignore_regex, &self.include_regex)?;
        hash_path(hash, &path, &filter, |h, p, st| {
            h.field("filename", p);
            // We hash only executable flag for files
            // as mode depends on the host system umask
            if st.is_file() {
                let mode = st.permissions().mode();
                let is_executable = mode & EXE_CHECK_MASK > 0;
                h.field("is_executable", is_executable);
            }
            hash_file_content(h, p, st)
                .map_err(|e| VersionError::Io(e, PathBuf::from(p)))?;
            Ok(())
        })?;
        Ok(())
    }
    fn build(&self, _guard: &mut Guard, _build: bool)
        -> Result<(), StepError>
    {
        Ok(())
    }
    fn is_dependent_on(&self) -> Option<&str> {
        None
    }
}

#[derive(RustcDecodable, Debug)]
pub struct Copy {
    pub source: PathBuf,
    pub path: PathBuf,
    pub owner_uid: Option<uid_t>,
    pub owner_gid: Option<gid_t>,
    pub umask: u32,
    pub preserve_permissions: bool,
    pub ignore_regex: Option<String>,
    pub include_regex: Option<String>,
    pub rules: Vec<String>,
    pub no_default_rules: Option<bool>,
}

impl Copy {
    pub fn config() -> V::Structure<'static> {
        V::Structure::new()
        .member("source", V::Scalar::new())
        .member("path", V::Directory::new().absolute(true))
        .member("ignore_regex", V::Scalar::new().optional())
        .member("include_regex", V::Scalar::new().optional())
        .member("rules", V::Sequence::new(V::Scalar::new()))
        .member("no_default_rules", V::Scalar::new().optional())
        .member("owner_uid", V::Numeric::new().min(0).optional())
        .member("owner_gid", V::Numeric::new().min(0).optional())
        .member("umask", V::Numeric::new().min(0).max(0o777).default(
            DEFAULT_UMASK as i64))
        .member("preserve_permissions", V::Scalar::new().default(false))
    }

    fn calc_mode(&self, stat: &Metadata) -> Option<u32> {
        if stat.file_type().is_symlink() {
            // ignore as we do not set permissions for symlinks
            return None;
        }
        if self.preserve_permissions {
            Some(stat.permissions().mode())
        } else {
            let base_mode = if stat.is_dir() {
                DIR_MODE
            } else {
                if stat.permissions().mode() & EXE_CHECK_MASK > 0 {
                    EXE_FILE_MODE
                } else {
                    FILE_MODE
                }
            };
            Some(base_mode & !self.umask)
        }
    }
}

impl BuildStep for Copy {
    fn name(&self) -> &'static str { "Copy" }
    fn hash(&self, _cfg: &Config, hash: &mut Digest)
        -> Result<(), VersionError>
    {
        let ref src = self.source;
        if src.starts_with("/work") {
            let filter = create_path_filter(&self.rules, self.no_default_rules,
                &self.ignore_regex, &self.include_regex)?;
            hash_path(hash, src, &filter, |h, p, st| {
                h.field("filename", p);
                h.opt_field("mode", &self.calc_mode(st));
                h.field("uid", self.owner_uid.unwrap_or(st.uid()));
                h.field("gid", self.owner_gid.unwrap_or(st.gid()));
                hash_file_content(h, p, st)
                    .map_err(|e| VersionError::Io(e, PathBuf::from(p)))?;
                Ok(())
            })?;
            hash.field("path", &self.path);
        } else {
            // We don't version the files outside of the /work because
            // we believe they are result of the commands run above
            //
            // And we need already built container to version the files
            // inside the container which is ugly
            hash.field("source", src);
            hash.field("path", &self.path);
            hash.field("preserve_permissions", self.preserve_permissions);
            if !self.preserve_permissions {
                hash.opt_field("owner_uid", &self.owner_uid);
                hash.opt_field("owner_gid", &self.owner_gid);
                hash.field("umask", self.umask);
            }
        }
        Ok(())
    }
    fn build(&self, _guard: &mut Guard, build: bool)
        -> Result<(), StepError>
    {
        if !build {
            return Ok(());
        }
        temporary_change_root("/vagga/root", || {
            let ref src = self.source;
            let ref dest = self.path;
            let ref typ = src.symlink_metadata()
                .map_err(|e| StepError::Read(src.into(), e))?;
            if typ.is_dir() {
                shallow_copy(src, typ, dest,
                        self.owner_uid, self.owner_gid,
                        self.calc_mode(typ))
                    .context((src, dest))?;
                let filter = create_path_filter(
                    &self.rules, self.no_default_rules,
                    &self.ignore_regex, &self.include_regex)?;
                let mut processed_paths = HashSet::new();
                filter.walk(src, |iter| {
                    for entry in iter {
                        let fpath = entry.path();
                        // We know that directory is inside
                        // the source
                        let path = fpath.strip_prefix(src).unwrap();
                        let parents: Vec<_> = path
                            .iter_self_and_parents()
                            .take_while(|p| !processed_paths.contains(*p))
                            .collect();
                        for parent in parents.iter().rev() {
                            let ref fdest = dest.join(parent);
                            let ref fsrc = src.join(parent);
                            let ref fsrc_stat = fsrc.symlink_metadata()
                                .map_err(|e| StepError::Read(src.into(), e))?;
                            shallow_copy(fsrc, fsrc_stat, fdest,
                                    self.owner_uid, self.owner_gid,
                                    self.calc_mode(fsrc_stat))
                                 .context((fsrc, fdest))?;
                            processed_paths.insert(PathBuf::from(parent));
                        }
                    }
                    Ok(())
                }).map_err(StepError::PathFilter).and_then(|x| x)?;
            } else {
                shallow_copy(src, typ, dest,
                        self.owner_uid, self.owner_gid,
                        self.calc_mode(typ))
                    .context((src, dest))?;
            }
            Ok(())
        })
    }
    fn is_dependent_on(&self) -> Option<&str> {
        None
    }
}

fn hash_path<F>(hash: &mut Digest, path: &Path, filter: &PathFilter, hash_file: F)
    -> Result<(), VersionError>
    where F: Fn(&mut Digest, &Path, &Metadata) -> Result<(), VersionError>
{
    match path.symlink_metadata() {
        Ok(ref meta) if meta.file_type().is_dir() => {
            hash_file(hash, path, meta)?;
            let all_rel_paths = get_sorted_rel_paths(path, filter)?;
            for rel_path in &all_rel_paths {
                let ref abs_path = path.join(rel_path);
                let stat = abs_path.symlink_metadata()
                    .map_err(|e| VersionError::Io(e, PathBuf::from(abs_path)))?;
                hash_file(hash, abs_path, &stat)?;
            }
        }
        Ok(ref meta) => {
            hash_file(hash, path, meta)?;
        }
        Err(ref e) if e.kind() == ErrorKind::NotFound => {
            return Err(VersionError::New);
        }
        Err(e) => {
            return Err(VersionError::Io(e, path.into()));
        }
    }
    Ok(())
}

fn get_sorted_rel_paths(path: &Path, filter: &PathFilter)
    -> Result<BTreeSet<PathBuf>, Vec<FilterError>>
{
    filter.walk(path, |iter| {
        let mut all_rel_paths = BTreeSet::new();
        for entry in iter {
            let fpath = entry.path();
            let rel_path = fpath.strip_prefix(path).unwrap();
            for parent in rel_path.iter_self_and_parents() {
                if !all_rel_paths.contains(parent) {
                    all_rel_paths.insert(PathBuf::from(parent));
                }
            }
        }
        all_rel_paths
    })
}

fn hash_file_content(hash: &mut Digest, path: &Path, stat: &Metadata)
    -> Result<(), io::Error>
{
    if stat.file_type().is_file() {
        let mut file = File::open(&path)?;
        hash.file(&path, &mut file)?;
    } else if stat.file_type().is_symlink() {
        let data = read_link(path)?;
        hash.field("symlink", data);
    }
    Ok(())
}

fn create_path_filter(rules: &Vec<String>, no_default_rules: Option<bool>,
    ignore_regex: &Option<String>, include_regex: &Option<String>)
    -> Result<PathFilter, String>
{
    if (!rules.is_empty() || no_default_rules.is_some()) &&
        (ignore_regex.is_some() || include_regex.is_some())
    {
        return Err(format!(
            "You must specify either rules or regular expressions \
             but not both"));
    }
    Ok(if !rules.is_empty() {
        let mut all_rules: Vec<&str> = vec!();
        if !no_default_rules.unwrap_or(false)  {
            all_rules.extend(DEFAULT_IGNORE_RULES);
        }
        for rule in rules {
            if !rule.starts_with('!') && !rule.starts_with('/') {
                return Err(format!(
                    "Relative paths are allowed only for excluding rules"));
            }
            all_rules.push(&rule);
        }
        PathFilter::glob(&all_rules[..])
    } else {
        PathFilter::regex(
            ignore_regex.as_ref().map(String::as_ref)
                .or(Some(DEFAULT_IGNORE_REGEX)),
            include_regex.as_ref())
    }.map_err(|e| format!("Can't compile copy filter: {}", e))?)
}
