#[macro_use]
extern crate clap;
extern crate cargo_metadata;
#[macro_use]
extern crate failure;
extern crate semver;

use cargo_metadata::DependencyKind;
use clap::{App, Arg, ArgMatches};
use failure::{Error, ResultExt, SyncFailure};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::exit;

#[allow(dead_code)]
struct Package {
    name: String,
    version: String,
    source: Option<String>,
    id: String,
    is_member: bool,
    include: bool,
    dependencies: Vec<Dependency>,
}

#[allow(dead_code)]
struct Dependency {
    index: Option<usize>,
    name: String,
    source: Option<String>,
    req: semver::VersionReq,
    kind: DependencyKind,
    optional: bool,
}

impl Package {
    fn from_metadata(
        pkg: &cargo_metadata::Package,
        members: &Vec<cargo_metadata::WorkspaceMember>,
    ) -> Result<Package, Error> {
        let mut is_member = false;
        let pkg_version = semver::Version::parse(&pkg.version)?;
        for member in members {
            // TODO: check source
            if member.name == pkg.name && member.version == pkg_version {
                is_member = true;
                break;
            }
        }
        let mut dependencies = Vec::new();
        for dep in &pkg.dependencies {
            dependencies.push(Dependency {
                index: None,
                name: dep.name.clone(),
                source: dep.source.clone(),
                req: dep.req.clone(),
                kind: dep.kind,
                optional: dep.optional,
            });
        }
        Ok(Package {
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            source: None,
            id: pkg.id.clone(),
            is_member,
            include: false,
            dependencies,
        })
    }

    fn mark_deps(
        &self,
        index: usize,
        packages: &Vec<Package>,
        include_set: &mut HashSet<usize>,
        ignore_set: &HashSet<usize>,
    ) {
        if ignore_set.contains(&index) {
            return;
        }
        include_set.insert(index);
        for dep in &self.dependencies {
            if let Some(dep_i) = dep.index {
                packages[dep_i].mark_deps(dep_i, packages, include_set, ignore_set);
            }
        }
    }
}

fn doit(matches: ArgMatches) -> Result<(), Error> {
    let manifest = matches.value_of("manifest-path").map(Path::new);
    // TODO: Specify features.
    let metadata = cargo_metadata::metadata_run(manifest, true, None)
        .map_err(SyncFailure::new)
        .context("Failed to load cargo metadata.")?;

    let mut packages: Vec<Package> = metadata
        .packages
        .iter()
        .map(|pkg| Package::from_metadata(pkg, &metadata.workspace_members))
        .collect::<Result<_, Error>>()?;
    let mut id_to_index: HashMap<String, usize> = HashMap::new();
    for (i, pkg) in packages.iter().enumerate() {
        if id_to_index.insert(pkg.id.clone(), i).is_some() {
            bail!("Duplicate key `{}`", pkg.id);
        }
    }
    let nodes = &metadata.resolve.as_ref().unwrap().nodes;
    for node in nodes {
        let index = id_to_index
            .get(&node.id)
            .ok_or_else(|| format_err!("Could not find resolve id `{}`", node.id))?;
        let pkg = &mut packages[*index];
        for mut dep in &mut pkg.dependencies {
            assert!(dep.index.is_none());
            for res_dep in &node.dependencies {
                // TODO: Support version (and maybe source).
                let name = res_dep.split(' ').next().expect("space separated id");
                if name == dep.name {
                    assert!(dep.index.is_none());
                    let dep_index = id_to_index
                        .get(res_dep)
                        .ok_or_else(|| format_err!("Could not find dep id `{}`", res_dep))?;
                    dep.index = Some(*dep_index);
                }
            }
        }
    }

    let mut ignore_set: HashSet<usize> = HashSet::new();
    if matches.is_present("exclude") {
        for exclude in matches.values_of("exclude").unwrap() {
            let mut found = false;
            for (i, pkg) in packages.iter().enumerate() {
                // TODO: Support full spec, at least the version.
                if pkg.name == exclude {
                    ignore_set.insert(i);
                    found = true;
                }
            }
            if !found {
                bail!("Could not find exclude spec `{}`.", exclude);
            }
        }
    }

    let mut root_set: HashSet<usize> = HashSet::new();
    if matches.is_present("package") {
        for include in matches.values_of("package").unwrap() {
            for (i, pkg) in packages.iter().enumerate() {
                // TODO: Support full spec, at least the version.
                if pkg.name == include {
                    root_set.insert(i);
                }
            }
        }
    } else {
        for (i, pkg) in packages.iter().enumerate() {
            if pkg.is_member {
                root_set.insert(i);
            }
        }
    }
    let mut include_set: HashSet<usize> = HashSet::new();
    for i in root_set {
        packages[i].mark_deps(i, &packages, &mut include_set, &ignore_set);
    }
    for i in include_set {
        packages[i].include = true;
    }

    println!("digraph dependencies {{");
    println!("  subgraph cluster0 {{");
    println!("  label = \"Workspace Members\";");
    for (i, pkg) in packages.iter().enumerate() {
        if pkg.is_member && pkg.include && !ignore_set.contains(&i) {
            println!("    N{} [label=\"{} {}\"];", i, pkg.name, pkg.version);
        }
    }
    println!("  }}");

    for (i, pkg) in packages.iter().enumerate() {
        // if !member_map.contains_key(&node.id) && !ignore_set.contains(&i) {
        if !pkg.is_member && pkg.include && !ignore_set.contains(&i) {
            println!("  N{} [label=\"{} {}\"];", i, pkg.name, pkg.version);
        }
        // }
    }
    for (i, pkg) in packages.iter().enumerate() {
        if pkg.include && !ignore_set.contains(&i) {
            for dep in &pkg.dependencies {
                if let Some(ref dep_i) = dep.index {
                    println!("  N{} -> N{};", i, dep_i);
                }
            }
        }
    }
    println!("}}");

    Ok(())
}

fn main() {
    let matches = App::new("cargo-dep")
        .version(crate_version!())
        // .bin_name("cargo")
        // .settings(&[AppSettings::GlobalVersion, AppSettings::SubcommandRequired])
        .about("Cargo dependency graph.")

        // .arg(
        //     Arg::with_name("verbose")
        //         .long("verbose")
        //         .short("v")
        //         .help("Verbose output"),
        // )
        .arg(
            Arg::with_name("manifest-path")
                .long("manifest-path")
                .value_name("PATH")
                .takes_value(true)
                .help("Path to Cargo.toml.")
        )
        .arg(
            Arg::with_name("package")
                .long("package")
                .short("p")
                .value_name("SPEC")
                .multiple(true)
                .help("Package name to include (default all).")
        )
        .arg(
            Arg::with_name("exclude")
                .long("exclude")
                .value_name("SPEC")
                .multiple(true)
                .help("Package name to exclude.")
        )
        .get_matches();

    if let Err(e) = doit(matches) {
        println!("Error: {}", e);
        for cause in e.causes().skip(1) {
            println!("Caused by: {}", cause);
        }
        exit(1);
    }
    exit(0)
}
