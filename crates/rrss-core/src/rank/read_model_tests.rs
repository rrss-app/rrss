use super::*;

struct Random(u64);
impl Random {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / ((1u64 << 53) as f64)
    }
}

#[test]
fn recovers_known_parameters_and_position_correction() {
    let mut rng = Random(0x123456789abcdef);
    let truth = [0.75, 1.0, -0.65, -0.6, 0.8, 1.2];
    let intercept = -1.15;
    let mut samples = Vec::new();
    for day in 0..60 {
        // Eight scroll sessions/day. Time order exposes the newest 40/60;
        // age and position correlate, but publication jitter gives overlap.
        for session in 0..8 {
            let mut candidates: Vec<_> = (0..60)
                .map(|_| {
                    let age = rng.next() * 3.0;
                    let f = [
                        rng.next() * 4.0 - 2.0,
                        rng.next() * 2.0 - 1.0,
                        f64::from(rng.next() < 0.7),
                        age,
                        (1.0 + (rng.next() * 4.0).floor()).ln(),
                        rng.next(),
                    ];
                    (age + rng.next() * 2.0, f, 2 + (rng.next() * 28.0) as i64)
                })
                .collect();
            candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (position, (_, features, mut feed)) in candidates.into_iter().take(40).enumerate() {
                if day % 7 == 0 && session == 0 && position == 0 {
                    feed = 0;
                }
                if position == 12 {
                    feed = 1;
                }
                let source = if feed == 0 {
                    4.0
                } else if feed == 1 {
                    -1.4
                } else {
                    0.0
                };
                let bucket = if position < 10 {
                    0
                } else if position < 30 {
                    1
                } else {
                    2
                };
                let z = intercept
                    + features.iter().zip(truth).map(|(x, b)| x * b).sum::<f64>()
                    + source
                    + [0.0, -1.0, -2.0][bucket];
                samples.push(Sample {
                    feed,
                    features,
                    position: Some(bucket as u8),
                    label: f64::from(rng.next() < 1.0 / (1.0 + (-z).exp())),
                    weight: 1.0,
                });
            }
        }
        // A few sidebar-only reads; selection bias is intentionally small here.
        samples.push(Sample {
            feed: 2,
            features: [1.0, 0.7, 1.0, 0.3, 1.0, 0.8],
            position: None,
            label: 1.0,
            weight: 1.0,
        });
    }
    let penalty = Penalty {
        prior: [1.0, 2.0, 0.0, 2.0, 2.0, 2.0],
        ..Default::default()
    };
    let fitted = fit(&samples, penalty.clone()).unwrap();
    let estimated: [f64; 6] = fitted.report.beta;
    for j in 0..6 {
        assert!(
            (estimated[j] / truth[j] - 1.0).abs() < 0.25,
            "{estimated:?}"
        );
    }
    let expected_intercept = intercept
        + truth
            .iter()
            .zip(fitted.mean)
            .map(|(b, m)| b * m)
            .sum::<f64>();
    assert!((fitted.intercept - expected_intercept).abs() < 0.3);
    assert!((fitted.position[0] + 1.0).abs() < 0.25 && (fitted.position[1] + 2.0).abs() < 0.25);
    assert!(fitted.sources[&0] > fitted.sources[&2] && fitted.sources[&2] > fitted.sources[&1]);
    for s in &mut samples {
        s.position = None;
    }
    let uncorrected = fit(&samples, penalty).unwrap();
    let error = |f: &Fit<6>| {
        (0..6)
            .map(|j| (f.report.beta[j] - truth[j]).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    assert!(error(&uncorrected) > error(&fitted) * 2.0);
    println!(
        "recovery n={} positive={} beta={estimated:?} source0={:.3} source1={:.3} position={:?} error={:.4} uncorrected_error={:.4}",
        samples.len(),
        fitted.report.positives,
        fitted.sources[&0],
        fitted.sources[&1],
        fitted.position,
        error(&fitted),
        error(&uncorrected)
    );
}

#[test]
fn small_sample_prior_and_fractional_labels() {
    let prior = [0.8, 0.5, -0.3, -0.7, 0.4, 0.9];
    let mut rng = Random(42);
    let samples: Vec<_> = (0..100)
        .map(|i| Sample {
            feed: i % 3,
            features: std::array::from_fn(|_| rng.next() * 2.0 - 1.0),
            position: None,
            label: if i < 20 { 0.75 } else { 0.0 },
            weight: 0.1,
        })
        .collect();
    let fitted = fit(
        &samples,
        Penalty {
            beta: 20.0,
            prior,
            ..Default::default()
        },
    )
    .unwrap();
    for (j, p) in prior.iter().enumerate() {
        assert!((fitted.report.beta[j] - p).abs() < 0.12);
    }
    println!("prior n=100 positive=20 beta={:?}", fitted.report.beta);
    let mut invalid = samples;
    invalid[0].label = f64::NAN;
    assert!(fit(&invalid, Penalty::default()).is_none());
}

#[test]
fn calibrated_prior_survives_unidentified_features() {
    let scores: Vec<_> = (0..500).map(|i| (i % 21) as f64 / 5.0 - 2.0).collect();
    let mut samples: Vec<_> = scores
        .iter()
        .map(|s| Sample {
            feed: 0,
            features: [1.0, 1.0, 1.0, 1.0, 1.0, 1.0 - 0.0002 * s],
            position: None,
            label: 1.0 / (1.0 + (0.4 - 1.7 * s).exp()),
            weight: 1.0,
        })
        .collect();
    let k = point_scale(&samples, &scores);
    assert!((k - 1.7).abs() < 0.01);
    let prior = [k, 2.0 * k, 0.0, 2.0 * k, 2.0 * k, 2.0 * k];
    let fitted = fit(
        &samples,
        Penalty {
            prior,
            ..Default::default()
        },
    )
    .unwrap();
    // Even an almost-constant feature perfectly correlated with the label
    // stays near its prior, preserving the direction on older candidates.
    assert_eq!(fitted.report.beta[..5], prior[..5]);
    assert!(fitted.report.stddev[5] < 0.001);
    assert!((fitted.report.beta[5] - prior[5]).abs() < 0.1);
    for j in [3, 5] {
        let mut old = [1.0; 6];
        old[j] = 0.0;
        assert!((fitted.contributions(0, old).0[j] + 2.0 * k).abs() < 0.1);
    }
    assert_eq!(point_scale(&samples[..49], &scores[..49]), 1.0);
    assert_eq!(point_scale(&samples, &vec![1.0; 500]), 1.0);
    for s in &mut samples {
        s.label = 1.0 - s.label;
    }
    assert_eq!(point_scale(&samples, &scores), 1.0);
    for s in &mut samples {
        s.label = 1.0;
    }
    assert_eq!(point_scale(&samples, &scores), 1.0);
    println!(
        "scale recovery n=500 truth=1.7 k={k:.6} constant_and_nearly_constant_gamma={:?}",
        fitted.report.beta
    );
}

#[test]
fn source_prior_centers_new_feeds_and_yields_to_their_outcomes() {
    // Feed 1: established, read half the time. Feed 2: newly subscribed, shown
    // eight times and never read. Feed 3: newly subscribed, never shown.
    let sample = |feed, label| Sample {
        feed,
        features: [0.0; 6],
        position: None,
        label,
        weight: 1.0,
    };
    let mut samples: Vec<_> = (0..200).map(|i| sample(1, f64::from(i % 2))).collect();
    samples.extend((0..8).map(|_| sample(2, 0.0)));
    let plain = fit(&samples, Penalty::default()).unwrap();
    let prior = fit(
        &samples,
        Penalty {
            source_prior: [(2, 1.5), (3, 1.5)].into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(plain.sources.get(&3), None);
    assert_eq!(prior.sources[&3], 1.5);
    // The prior lifts the skipped new feed, but its skips still pull it below
    // the prior center and below the established feed's neutral effect.
    assert!(prior.sources[&2] > plain.sources[&2]);
    assert!(prior.sources[&2] < 1.5);
    println!(
        "new feed effect: plain {:.3}, prior {:.3}",
        plain.sources[&2], prior.sources[&2]
    );
}

#[test]
fn source_uncertainty_shrinks_with_opportunities_and_daily_draws_repeat() {
    // Feeds 1 and 2 read at the same rate; feed 2 has a tenth of the samples.
    let sample = |feed, label| Sample {
        feed,
        features: [0.0; 6],
        position: None,
        label,
        weight: 1.0,
    };
    let mut samples: Vec<_> = (0..1000)
        .map(|i| sample(1, f64::from(i % 4 == 0)))
        .collect();
    samples.extend((0..100).map(|i| sample(2, f64::from(i % 4 == 0))));
    let fitted = fit(
        &samples,
        Penalty {
            source_prior: [(3, 1.5)].into(),
            ..Default::default()
        },
    )
    .unwrap();
    let sd = &fitted.source_sd;
    // Given the intercept: 1 / sqrt(1 + n p (1 - p)), p = 1/4 at the optimum.
    assert!(
        (sd[&1] - (1.0 + 1000.0 * 0.1875_f64).recip().sqrt()).abs() < 0.002,
        "{sd:?}"
    );
    assert!(
        (sd[&2] - (1.0 + 100.0 * 0.1875_f64).recip().sqrt()).abs() < 0.005,
        "{sd:?}"
    );
    // A prior-only feed keeps the prior's spread (lambda 1).
    assert_eq!(sd[&3], 1.0);
    // One draw per feed and day: stable within the day, close to N(0, 1) across days.
    assert_eq!(daily_normal(4, 20_000), daily_normal(4, 20_000));
    assert_ne!(daily_normal(4, 20_000), daily_normal(5, 20_000));
    let draws: Vec<_> = (0..20_000).map(|day| daily_normal(7, day)).collect();
    let mean = draws.iter().sum::<f64>() / draws.len() as f64;
    let var = draws.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / draws.len() as f64;
    assert!(
        mean.abs() < 0.03 && (var - 1.0).abs() < 0.05,
        "mean {mean} var {var}"
    );
    println!("source sd: {sd:?}");
}
