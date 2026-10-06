//! Catch dependency upgrades whose redistribution notices were not refreshed.
#[test]
fn locked_dependencies_have_license_notices() {
    let notices = include_str!("../THIRD_PARTY_NOTICES.txt");
    for package in include_str!("../Cargo.lock").split("[[package]]").skip(1) {
        if !package.lines().any(|l| l.starts_with("source = ")) {
            continue;
        }
        let field = |key: &str| package.lines().find_map(|l| l.strip_prefix(key)).unwrap().trim_matches('"');
        let name = field("name = ");
        let version = field("version = ");
        let prefix = format!("component: {name} {version} | declared: ");
        let entry = notices
            .lines()
            .find(|l| l.starts_with(&prefix))
            .unwrap_or_else(|| panic!("refresh notices for {name} {version}"));
        let (_, ids) = entry.split_once(" | notices: ").unwrap();
        assert!(!ids.is_empty());
        for id in ids.split(", ") {
            assert!(notices.contains(&format!("=== notice {id} ===")));
        }
    }
}
