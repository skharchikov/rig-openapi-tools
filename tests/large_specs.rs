use rig_openapi_tools::OpenApiToolset;

#[test]
fn parse_petstore_spec() {
    let toolset = OpenApiToolset::from_file("examples/petstore.json").unwrap();
    println!("Petstore: {} tools", toolset.len());
    assert_eq!(toolset.len(), 19);
}

/// This test requires examples/stripe.json to be present.
/// Download it with:
///   curl -sL -o examples/stripe.json https://raw.githubusercontent.com/stripe/openapi/master/openapi/spec3.json
#[test]
#[ignore]
fn parse_stripe_spec() {
    let toolset = OpenApiToolset::from_file("examples/stripe.json").unwrap();
    println!("Stripe: {} tools", toolset.len());
    assert!(toolset.len() > 100, "Stripe should have 100+ operations");
}
