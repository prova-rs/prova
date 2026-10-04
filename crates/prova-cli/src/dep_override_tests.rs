use super::*;

fn git(tag: Option<&str>, rev: Option<&str>) -> PackageSource {
    PackageSource::Detailed(PackageDetail {
        git: Some("https://example.invalid/standards".into()),
        tag: tag.map(String::from),
        rev: rev.map(String::from),
        module: Some("src/standards.lua".into()),
        ..PackageDetail::default()
    })
}

fn deps() -> BTreeMap<String, PackageSource> {
    BTreeMap::from([
        ("project-standards".to_string(), git(Some("v1.3.0"), None)),
        ("fixtures".to_string(), PackageSource::Path("./fixtures".into())),
    ])
}

#[test]
fn a_bare_spec_reads_as_tag_rev_or_path_and_prefixes_settle_doubt() {
    assert_eq!(parse("a=v1.4.0", "--dep").unwrap().pin, Pin::Tag("v1.4.0".into()));
    assert_eq!(parse("a=3f2c9a1", "--dep").unwrap().pin, Pin::Rev("3f2c9a1".into()));
    assert_eq!(parse("a=../std", "--dep").unwrap().pin, Pin::Path("../std".into()));
    assert_eq!(parse("a=tag:3f2c9a1", "--dep").unwrap().pin, Pin::Tag("3f2c9a1".into()));
    assert_eq!(parse("a=path:std", "--dep").unwrap().pin, Pin::Path("std".into()));
    assert!(parse("a=", "--dep").is_err());
    assert!(parse("=v1", "--dep").is_err());
    assert!(parse("a=tag:", "--dep").is_err());
}

#[test]
fn a_tag_override_keeps_the_git_url_and_module_and_names_both_pins() {
    let mut d = deps();
    let applied = apply(&mut d, &[parse("project-standards=v1.4.0", "--dep").unwrap()]).unwrap();
    let PackageSource::Detailed(detail) = &d["project-standards"] else { panic!("still git") };
    assert_eq!(detail.git.as_deref(), Some("https://example.invalid/standards"));
    assert_eq!(detail.module.as_deref(), Some("src/standards.lua"));
    assert_eq!((detail.tag.as_deref(), detail.rev.as_deref()), (Some("v1.4.0"), None));
    assert_eq!(applied[0].line(), "project-standards: tag v1.3.0 -> tag v1.4.0 (--dep)");
}

#[test]
fn a_rev_override_clears_the_tag_and_a_path_override_replaces_the_source() {
    let mut d = deps();
    apply(&mut d, &[parse("project-standards=rev:0123456789abcdef", "--dep").unwrap()]).unwrap();
    let PackageSource::Detailed(detail) = &d["project-standards"] else { panic!("still git") };
    assert_eq!((detail.tag.as_deref(), detail.rev.as_deref()), (None, Some("0123456789abcdef")));
    apply(&mut d, &[parse("fixtures=path:/tmp/fx", "--dep").unwrap()]).unwrap();
    assert_eq!(d["fixtures"], PackageSource::Path("/tmp/fx".into()));
}

#[test]
fn an_unknown_name_and_a_tag_on_a_path_source_are_refused_with_the_fix() {
    let mut d = deps();
    let err = apply(&mut d, &[parse("nope=v1", "--dep").unwrap()]).unwrap_err();
    assert!(err.contains("[fixtures, project-standards]") && err.contains("-P name=path"), "{err}");
    let err = apply(&mut d, &[parse("fixtures=v2", "--dep").unwrap()]).unwrap_err();
    assert!(err.contains("not a git source") && err.contains("--dep fixtures=path:<dir>"), "{err}");
    assert_eq!(d, deps(), "a refused override changes nothing");
}

#[test]
fn env_overrides_match_by_env_spelling_and_the_cli_wins_over_them() {
    let env = from_env(
        [
            ("PROVA_DEP_PROJECT_STANDARDS".to_string(), "v1.4.0".to_string()),
            ("UNRELATED".to_string(), "x".to_string()),
        ]
        .into_iter(),
    )
    .unwrap();
    assert_eq!(env.len(), 1);
    let mut d = deps();
    let mut all = env.clone();
    all.push(parse("project-standards=v1.5.0", "--dep").unwrap());
    let applied = apply(&mut d, &all).unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].line(), "project-standards: tag v1.3.0 -> tag v1.5.0 (--dep)");
    let only_env = apply(&mut deps(), &env).unwrap();
    assert_eq!(only_env[0].origin, "PROVA_DEP_PROJECT_STANDARDS");
}
