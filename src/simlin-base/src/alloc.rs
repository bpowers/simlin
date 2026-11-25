// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/// Complementary error function approximation using Abramowitz & Stegun 26.2.17.
/// For z >= 0, erfc(z) ~ P(t) * exp(-z^2) where t = 1/(1 + p*z).
/// Maximum error |epsilon(z)| <= 1.5e-7.
pub fn erfc_approx(z: f64) -> f64 {
    if z < 0.0 {
        return 2.0 - erfc_approx(-z);
    }
    let a1: f64 = 0.254829592;
    let a2: f64 = -0.284496736;
    let a3: f64 = 1.421413741;
    let a4: f64 = -1.453152027;
    let a5: f64 = 1.061405429;
    let p: f64 = 0.3275911;

    let t = 1.0 / (1.0 + p * z);
    (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-z * z).exp()
}

/// Standard normal CDF: Phi(x) = P(X <= x) for X ~ N(0,1).
/// Computed as 0.5 * erfc(-x / sqrt(2)).
pub fn normal_cdf(x: f64) -> f64 {
    if x.is_nan() {
        return f64::NAN;
    }
    0.5 * erfc_approx(-x / std::f64::consts::SQRT_2)
}

/// Compute the allocation curve value q_i(p) for a single requester.
///
/// Given a "market price" p and a priority profile (ptype, ppriority, pwidth, pextra),
/// returns the fraction of the request that should be allocated, scaled by request amount.
///
/// The price p decreases as supply tightens. Higher ppriority means higher priority,
/// so requesters with higher ppriority maintain their allocation at lower prices.
/// The survival function (1 - CDF) is used: fraction = P(priority >= p).
pub fn alloc_curve(
    p: f64,
    request: f64,
    ptype: i32,
    ppriority: f64,
    pwidth: f64,
    pextra: f64,
) -> f64 {
    if request <= 0.0 {
        return 0.0;
    }
    let fraction = match ptype % 10 {
        0 => {
            // Fixed quantity: allocated when price is at or below priority
            if p <= ppriority { 1.0 } else { 0.0 }
        }
        1 => {
            // Rectangular: survival function of uniform distribution
            let half_width = pwidth;
            let lo = ppriority - half_width;
            let hi = ppriority + half_width;
            if p <= lo {
                1.0
            } else if p >= hi {
                0.0
            } else {
                (hi - p) / (hi - lo)
            }
        }
        2 => {
            // Triangular: survival function of triangular distribution
            let half_width = pwidth;
            let lo = ppriority - half_width;
            let hi = ppriority + half_width;
            if p <= lo {
                1.0
            } else if p >= hi {
                0.0
            } else if p <= ppriority {
                let t = (hi - p) / (hi - lo);
                1.0 - 2.0 * (1.0 - t) * (1.0 - t)
            } else {
                let t = (hi - p) / (hi - lo);
                2.0 * t * t
            }
        }
        3 => {
            // Normal: survival function P(X >= p) where X ~ N(ppriority, pwidth^2)
            if pwidth <= 0.0 {
                if p <= ppriority { 1.0 } else { 0.0 }
            } else {
                normal_cdf((ppriority - p) / pwidth)
            }
        }
        4 => {
            // Exponential: survival function of symmetric exponential
            if pwidth <= 0.0 {
                if p <= ppriority { 1.0 } else { 0.0 }
            } else {
                let z = (p - ppriority) / pwidth;
                if z > 0.0 {
                    0.5 * (-z).exp()
                } else {
                    1.0 - 0.5 * z.exp()
                }
            }
        }
        5 => {
            // Constant Elasticity of Substitution (CES)
            if p <= 0.0 {
                1.0
            } else if ppriority <= 0.0 {
                0.0
            } else {
                let ratio = ppriority / p;
                let q = ratio.powf(pextra);
                if q.is_infinite() { 1.0 } else { q / (1.0 + q) }
            }
        }
        _ => {
            if p <= ppriority {
                1.0
            } else {
                0.0
            }
        }
    };
    let alloc = request * fraction;
    if ptype >= 10 { alloc.floor() } else { alloc }
}

/// Perform the ALLOCATE AVAILABLE computation across all requesters.
///
/// Uses bisection search to find the market-clearing "price" p such that
/// the sum of all allocations equals the available supply (or total demand
/// if supply exceeds it).
pub fn allocate_available(
    requests: &[f64],
    profiles: &[(f64, f64, f64, f64)],
    avail: f64,
) -> Vec<f64> {
    let n = requests.len();
    if n == 0 {
        return vec![];
    }

    let total_demand: f64 = requests.iter().filter(|r| **r > 0.0).sum();
    if avail >= total_demand {
        return requests.iter().map(|&r| r.max(0.0)).collect();
    }
    if avail <= 0.0 {
        return vec![0.0; n];
    }

    // Find the search range from priority values
    let mut p_min = f64::INFINITY;
    let mut p_max = f64::NEG_INFINITY;
    for (ptype, ppriority, pwidth, _pextra) in profiles.iter() {
        let pt = (*ptype as i32) % 10;
        let spread = match pt {
            0 => 1.0,
            1 | 2 => *pwidth,
            3 => pwidth * 6.0,
            4 => pwidth * 10.0,
            5 => ppriority * 10.0,
            _ => 1.0,
        };
        p_min = p_min.min(ppriority - spread);
        p_max = p_max.max(ppriority + spread);
    }

    // Bisection search for the market-clearing price
    let mut lo = p_min;
    let mut hi = p_max;
    for _ in 0..100 {
        let mid = (lo + hi) / 2.0;
        let total: f64 = (0..n)
            .map(|i| {
                let (pt, pp, pw, pe) = profiles[i];
                alloc_curve(mid, requests[i], pt as i32, pp, pw, pe)
            })
            .sum();
        if total < avail {
            hi = mid;
        } else {
            lo = mid;
        }
        if (hi - lo).abs() < 1e-14 * (1.0 + hi.abs()) {
            break;
        }
    }

    let p_star = (lo + hi) / 2.0;
    (0..n)
        .map(|i| {
            let (pt, pp, pw, pe) = profiles[i];
            alloc_curve(p_star, requests[i], pt as i32, pp, pw, pe)
        })
        .collect()
}
