use blyte_provider::{pricing_on, Pricing};

#[test]
fn hydrogen_price() {
    let expected = Some(Pricing {
        input: 1.50,
        cached: 0.075,
        output: 7.50,
    });
    assert_eq!(
        pricing_on("gemini/gemini-4-hydrogen", (2026, 11, 19)),
        expected
    );
    assert_eq!(
        pricing_on("gemini/gemini-4-hydrogen", (2027, 1, 1)),
        expected
    );
}

#[test]
fn neon_price() {
    let expected = Some(Pricing {
        input: 3.00,
        cached: 0.15,
        output: 15.00,
    });
    assert_eq!(pricing_on("gemini-4-neon", (2026, 11, 19)), expected);
    assert_eq!(pricing_on("gemini-4-neon", (2027, 1, 1)), expected);
}

#[test]
fn argon_introductory_price_ends_2026_11_30() {
    assert_eq!(
        pricing_on("gemini-4-argon", (2026, 11, 30)),
        Some(Pricing {
            input: 2.00,
            cached: 0.10,
            output: 10.00,
        })
    );
    assert_eq!(
        pricing_on("gemini-4-argon", (2026, 12, 1)),
        Some(Pricing {
            input: 4.00,
            cached: 0.20,
            output: 20.00,
        })
    );
}

#[test]
fn carbon_introductory_price_ends_2026_11_30() {
    assert_eq!(
        pricing_on("vertex/gemini-4-carbon", (2026, 11, 30)),
        Some(Pricing {
            input: 2.00,
            cached: 0.10,
            output: 10.00,
        })
    );
    assert_eq!(
        pricing_on("vertex/gemini-4-carbon", (2026, 12, 1)),
        Some(Pricing {
            input: 4.00,
            cached: 0.20,
            output: 20.00,
        })
    );
}

#[test]
fn flash_price_unchanged() {
    assert_eq!(
        pricing_on("gemini-3.8-flash-high", (2026, 11, 19)),
        Some(Pricing {
            input: 0.75,
            cached: 0.075,
            output: 3.75,
        })
    );
}

#[test]
fn unknown_model_has_no_price() {
    assert_eq!(pricing_on("gemini-4-xenon", (2026, 11, 19)), None);
}
