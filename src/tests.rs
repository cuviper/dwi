use super::*;

#[test]
fn kernel_cli_accepts_running_or_explicit_release() {
    let args = Args::try_parse_from(["dwi", "--kernel"]).unwrap();
    assert_eq!(args.kernel, Some(None));
    assert!(args.file.is_none());
    for argv in [
        vec!["dwi", "--kernel", "7.3.4-test"],
        vec!["dwi", "--kernel=7.3.4-test"],
    ] {
        assert_eq!(
            Args::try_parse_from(argv).unwrap().kernel,
            Some(Some("7.3.4-test".into()))
        );
    }
    let args = Args::try_parse_from(["dwi", "--kernel", "--side-by-side"]).unwrap();
    assert_eq!(args.kernel, Some(None));
    assert!(args.side_by_side);
    let args = Args::try_parse_from(["dwi", "program"]).unwrap();
    assert_eq!(args.file, Some(PathBuf::from("program")));
    assert!(args.kernel.is_none());
}

#[test]
fn kernel_and_file_inputs_are_mutually_exclusive_and_required() {
    assert!(Args::try_parse_from(["dwi"]).is_err());
    assert!(Args::try_parse_from(["dwi", "program", "--kernel"]).is_err());
    assert!(Args::try_parse_from(["dwi", "--kernel=7.3.4-test", "program"]).is_err());
    assert!(Args::try_parse_from(["dwi", "program", "--module", "dummy"]).is_err());
    assert!(Args::try_parse_from(["dwi", "--module"]).is_err());
}

#[test]
fn module_cli_accepts_default_or_explicit_kernel_release() {
    let args = Args::try_parse_from(["dwi", "--module", "dummy"]).unwrap();
    assert_eq!(args.module.as_deref(), Some("dummy"));
    assert!(args.kernel.is_none());
    let args =
        Args::try_parse_from(["dwi", "--kernel", "7.3.4-test", "--module", "dummy"]).unwrap();
    assert_eq!(args.kernel, Some(Some("7.3.4-test".into())));
    assert_eq!(args.module.as_deref(), Some("dummy"));
    let args = Args::try_parse_from(["dwi", "--kernel", "--module", "dummy"]).unwrap();
    assert_eq!(args.kernel, Some(None));
    assert_eq!(args.module.as_deref(), Some("dummy"));
}
