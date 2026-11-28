use dagaz_protocol::Quality;
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    existing: Quality,
    requested: Quality,
    force: bool,
    reuse: bool,
}

#[test]
fn shared_download_quality_contract() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../fixtures/contract/download-policy.json"
    ))
    .unwrap();
    let rank = |q| match q {
        Quality::Original => 0,
        Quality::High => 1,
        Quality::Low => 2,
    };
    assert!(cases.len() >= 6);
    for case in cases {
        assert_eq!(
            case.reuse,
            if case.force {
                case.existing == case.requested
            } else {
                rank(case.existing) <= rank(case.requested)
            }
        );
    }
}
