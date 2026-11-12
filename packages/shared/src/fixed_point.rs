//! Deterministic fixed-point decimal parsing and coffee unit conversions.
//!
//! All functions in this module operate strictly using checked `u128` integer
//! arithmetic (zero floating-point operations) so results remain bit-identical
//! across native hosts and WASI targets.

use core::fmt;

/// Decimal places for high-precision parsed intermediate prices (`10^12`).
pub const FIXED_POINT_SCALE_E12_DECIMALS: u8 = 12;

/// Scaling factor for 12-decimal fixed-point intermediate values (`10^12`).
pub const FIXED_POINT_SCALE_E12: u128 = 1_000_000_000_000;

/// Decimal places for FX rates in BRL per USD (`10^8`).
pub const FIXED_POINT_SCALE_E8_DECIMALS: u8 = 8;

/// Scaling factor for 8-decimal fixed-point USD/kg prices and FX rates (`10^8`).
pub const FIXED_POINT_SCALE_E8: u128 = 100_000_000;

/// Exact SI pound mass numerator in `10^-8` kg (`1 lb = 0.45359237 kg`).
pub const SI_GRAMS_PER_POUND_E8: u128 = 45_359_237;

/// Standard Brazilian CEPEA green-coffee bag weight in kilograms.
pub const CEPEA_BAG_WEIGHT_KG: u128 = 60;

/// Maximum fractional digits accepted by [`parse_decimal_e12_round_half_up`].
///
/// Public economic datasets such as IMF PCPS JSON emit IEEE/decimal strings with 13–17
/// significant digits (e.g., `"330.1219047619048"`). Accepting up to 24 fractional digits
/// preserves deterministic integer parsing while failing closed on malformed or pathological payloads.
pub const MAX_HIGH_PRECISION_DECIMAL_FRAC_DIGITS: usize = 24;

/// Errors returned by deterministic fixed-point parsing and unit conversions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixedPointError {
    /// Input string is empty or whitespace-only.
    EmptyInput,
    /// Input contains a negative sign; negative prices are rejected.
    NegativeValueNotAllowed,
    /// Input contains exponent notation (`e` or `E`), which is rejected.
    ScientificNotationNotAllowed,
    /// Input does not conform to strict unsigned base-10 decimal syntax.
    InvalidDecimalFormat { reason: &'static str },
    /// Input contains more fractional digits than the target scale permits.
    ExcessiveFractionalPrecision {
        max_digits: u8,
        actual_digits: usize,
    },
    /// Division or FX rate denominator was zero.
    ZeroDenominator,
    /// Checked integer arithmetic exceeded `u128::MAX`.
    Overflow,
}

impl fmt::Display for FixedPointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "decimal input cannot be empty"),
            Self::NegativeValueNotAllowed => {
                write!(f, "negative values are not permitted")
            }
            Self::ScientificNotationNotAllowed => {
                write!(f, "scientific notation is not permitted")
            }
            Self::InvalidDecimalFormat { reason } => {
                write!(f, "invalid decimal format: {}", reason)
            }
            Self::ExcessiveFractionalPrecision {
                max_digits,
                actual_digits,
            } => write!(
                f,
                "fractional precision of {} digits exceeds maximum {} digits",
                actual_digits, max_digits
            ),
            Self::ZeroDenominator => {
                write!(f, "denominator / FX rate must be greater than zero")
            }
            Self::Overflow => write!(f, "fixed-point arithmetic overflow (exceeded u128::MAX)"),
        }
    }
}

impl std::error::Error for FixedPointError {}

/// Parse an unsigned decimal string into an integer scaled by `10^12`.
///
/// Rejects empty strings, whitespace, explicit signs (`+`/`-`), thousands separators,
/// scientific notation, leading/trailing dots (`.5`, `5.`), more than 12 fractional
/// digits, and values overflowing `u128`.
pub fn parse_decimal_e12(input: &str) -> Result<u128, FixedPointError> {
    parse_unsigned_decimal_scaled(input, FIXED_POINT_SCALE_E12_DECIMALS)
}

/// Parse an unsigned decimal string (such as an FX rate) into an integer scaled by `10^8`.
pub fn parse_decimal_e8(input: &str) -> Result<u128, FixedPointError> {
    parse_unsigned_decimal_scaled(input, FIXED_POINT_SCALE_E8_DECIMALS)
}

/// Parse an unsigned decimal string with up to [`MAX_HIGH_PRECISION_DECIMAL_FRAC_DIGITS`]
/// fractional digits into an integer scaled by `10^12`, applying exact decimal round-half-up
/// at the 13th fractional digit (`>= '5'` rounds up).
///
/// Rejects empty strings, whitespace, signs (`+`/`-`), separators, scientific notation,
/// leading/trailing dots, fractional precision exceeding `24` digits, and `u128` overflow.
pub fn parse_decimal_e12_round_half_up(input: &str) -> Result<u128, FixedPointError> {
    let (int_part, frac_part) = split_and_validate_unsigned_decimal(input)?;

    if frac_part.len() <= FIXED_POINT_SCALE_E12_DECIMALS as usize {
        return assemble_scaled_decimal(int_part, frac_part, FIXED_POINT_SCALE_E12_DECIMALS);
    }

    if frac_part.len() > MAX_HIGH_PRECISION_DECIMAL_FRAC_DIGITS {
        return Err(FixedPointError::ExcessiveFractionalPrecision {
            max_digits: MAX_HIGH_PRECISION_DECIMAL_FRAC_DIGITS as u8,
            actual_digits: frac_part.len(),
        });
    }

    let (keep_12, remainder) = frac_part.split_at(FIXED_POINT_SCALE_E12_DECIMALS as usize);
    let base_e12 = assemble_scaled_decimal(int_part, keep_12, FIXED_POINT_SCALE_E12_DECIMALS)?;

    // In base-10, 0.<remainder> >= 0.5 holds if and only if the first discarded digit >= '5'.
    if remainder.as_bytes()[0] >= b'5' {
        base_e12.checked_add(1).ok_or(FixedPointError::Overflow)
    } else {
        Ok(base_e12)
    }
}

/// Shared strict base-10 unsigned decimal parser for arbitrary fixed-point scale `10^decimals`.
fn parse_unsigned_decimal_scaled(input: &str, decimals: u8) -> Result<u128, FixedPointError> {
    let (int_part, frac_part) = split_and_validate_unsigned_decimal(input)?;

    if frac_part.len() > decimals as usize {
        return Err(FixedPointError::ExcessiveFractionalPrecision {
            max_digits: decimals,
            actual_digits: frac_part.len(),
        });
    }

    assemble_scaled_decimal(int_part, frac_part, decimals)
}

fn split_and_validate_unsigned_decimal(input: &str) -> Result<(&str, &str), FixedPointError> {
    if input.trim().is_empty() {
        return Err(FixedPointError::EmptyInput);
    }

    if input.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err(FixedPointError::InvalidDecimalFormat {
            reason: "whitespace is not permitted",
        });
    }

    if input.starts_with('-') {
        return Err(FixedPointError::NegativeValueNotAllowed);
    }

    if input.starts_with('+') {
        return Err(FixedPointError::InvalidDecimalFormat {
            reason: "leading '+' sign is not permitted",
        });
    }

    if input.contains(['e', 'E']) {
        return Err(FixedPointError::ScientificNotationNotAllowed);
    }

    if input.contains([',', '_']) {
        return Err(FixedPointError::InvalidDecimalFormat {
            reason: "thousands or digit separators are not permitted",
        });
    }

    let (int_part, frac_part) = match input.split_once('.') {
        Some((int_str, frac_str)) => {
            if int_str.is_empty() {
                return Err(FixedPointError::InvalidDecimalFormat {
                    reason: "integer part before decimal point cannot be empty",
                });
            }
            if frac_str.is_empty() {
                return Err(FixedPointError::InvalidDecimalFormat {
                    reason: "fractional part after decimal point cannot be empty",
                });
            }
            if frac_str.contains('.') {
                return Err(FixedPointError::InvalidDecimalFormat {
                    reason: "multiple decimal points are not permitted",
                });
            }
            (int_str, frac_str)
        }
        None => (input, ""),
    };

    if !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(FixedPointError::InvalidDecimalFormat {
            reason: "only ASCII digits 0-9 and at most one decimal point are permitted",
        });
    }

    Ok((int_part, frac_part))
}

fn assemble_scaled_decimal(
    int_part: &str,
    frac_part: &str,
    decimals: u8,
) -> Result<u128, FixedPointError> {
    let scale_factor = 10u128
        .checked_pow(decimals as u32)
        .ok_or(FixedPointError::Overflow)?;

    let int_val = parse_ascii_digits_u128(int_part)?;
    let int_scaled = int_val
        .checked_mul(scale_factor)
        .ok_or(FixedPointError::Overflow)?;

    let frac_val = if frac_part.is_empty() {
        0u128
    } else {
        let raw_frac = parse_ascii_digits_u128(frac_part)?;
        let pad_pow = (decimals as usize)
            .checked_sub(frac_part.len())
            .ok_or(FixedPointError::Overflow)? as u32;
        let pad_factor = 10u128
            .checked_pow(pad_pow)
            .ok_or(FixedPointError::Overflow)?;
        raw_frac
            .checked_mul(pad_factor)
            .ok_or(FixedPointError::Overflow)?
    };

    int_scaled
        .checked_add(frac_val)
        .ok_or(FixedPointError::Overflow)
}

fn parse_ascii_digits_u128(digits: &str) -> Result<u128, FixedPointError> {
    let mut acc: u128 = 0;
    for byte in digits.bytes() {
        let d = (byte - b'0') as u128;
        acc = acc
            .checked_mul(10)
            .and_then(|v| v.checked_add(d))
            .ok_or(FixedPointError::Overflow)?;
    }
    Ok(acc)
}

/// Overflow-safe unsigned integer division with half-up rounding (`>= 0.5` rounds up).
///
/// Uses quotient and remainder (`r >= den - den / 2`, equivalent to `2 * r >= den`)
/// rather than `(numerator + denominator / 2) / denominator`, avoiding premature overflow
/// when `numerator` approaches `u128::MAX`.
pub fn round_half_up_div_u128(numerator: u128, denominator: u128) -> Result<u128, FixedPointError> {
    if denominator == 0 {
        return Err(FixedPointError::ZeroDenominator);
    }

    let quotient = numerator / denominator;
    let remainder = numerator % denominator;

    // Exact ceiling(denominator / 2) computed without `denominator + 1` or `2 * remainder`.
    let half_up_threshold = denominator - (denominator / 2);
    if remainder >= half_up_threshold {
        quotient.checked_add(1).ok_or(FixedPointError::Overflow)
    } else {
        Ok(quotient)
    }
}

/// Convert a price from generic `USc/lb` (scaled by `10^12`) to `USD/kg` (scaled by `10^8`).
///
/// Derivation:
/// - Input: `x = price_{USc/lb} * 10^12`
/// - USc to USD (`1 USD = 100 USc`): `price_{USD/lb} = x / 10^14`
/// - Exact SI pound to kg (`1 lb = 45_359_237 / 10^8 kg`):
///   `price_{USD/kg} = (x / 10^14) * (10^8 / 45_359_237)`
/// - Scale output by `10^8` (`USD/kg_e8`):
///   `price_{USD/kg_e8} = price_{USD/kg} * 10^8 = (x * 100) / 45_359_237`
pub fn convert_usc_per_lb_e12_to_usd_per_kg_e8(
    usc_per_lb_e12: u128,
) -> Result<u128, FixedPointError> {
    let numerator = usc_per_lb_e12
        .checked_mul(100)
        .ok_or(FixedPointError::Overflow)?;
    round_half_up_div_u128(numerator, SI_GRAMS_PER_POUND_E8)
}

/// Convert ICE Coffee C price from `USc/lb` (scaled by `10^12`) to `USD/kg` (scaled by `10^8`).
///
/// Backwards-compatible alias delegating to [`convert_usc_per_lb_e12_to_usd_per_kg_e8`].
pub fn convert_ice_usc_per_lb_e12_to_usd_per_kg_e8(
    usc_per_lb_e12: u128,
) -> Result<u128, FixedPointError> {
    convert_usc_per_lb_e12_to_usd_per_kg_e8(usc_per_lb_e12)
}

/// Convert CEPEA spot coffee price from `BRL per 60 kg bag` (scaled by `10^12`) and an FX rate
/// in `BRL per USD` (scaled by `10^8`) to `USD/kg` (scaled by `10^8`).
///
/// Derivation:
/// - Input price: `b = price_{BRL/bag} * 10^12`
/// - Input FX: `f = fx_{BRL/USD} * 10^8` (`BRL` per 1 `USD`, non-zero)
/// - Bag to kg (`1 bag = 60 kg`): `price_{BRL/kg} = b / (60 * 10^12)`
/// - Currency conversion (`price_{USD/kg} = price_{BRL/kg} / fx_{BRL/USD}`):
///   `price_{USD/kg} = (b / (60 * 10^12)) / (f / 10^8) = (b * 10^8) / (60 * f * 10^12)`
/// - Scale output by `10^8` (`USD/kg_e8`):
///   `price_{USD/kg_e8} = (b * 10^16) / (60 * f * 10^12) = (b * 10_000) / (60 * f)`
pub fn convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(
    brl_per_bag_e12: u128,
    fx_brl_per_usd_e8: u128,
) -> Result<u128, FixedPointError> {
    if fx_brl_per_usd_e8 == 0 {
        return Err(FixedPointError::ZeroDenominator);
    }

    let numerator = brl_per_bag_e12
        .checked_mul(10_000)
        .ok_or(FixedPointError::Overflow)?;
    let denominator = CEPEA_BAG_WEIGHT_KG
        .checked_mul(fx_brl_per_usd_e8)
        .ok_or(FixedPointError::Overflow)?;

    round_half_up_div_u128(numerator, denominator)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format_display_price;

    #[test]
    fn test_vector_ice_coffee_c_usc_per_lb_to_usd_per_kg() {
        let parsed_e12 = parse_decimal_e12("330.1219047619").expect("valid ICE decimal input");
        assert_eq!(parsed_e12, 330_121_904_761_900u128);

        let usd_per_kg_e8 =
            convert_ice_usc_per_lb_e12_to_usd_per_kg_e8(parsed_e12).expect("valid ICE conversion");
        assert_eq!(usd_per_kg_e8, 727_794_219u128);
        assert_eq!(format_display_price(usd_per_kg_e8, 8), "7.27794219");
    }

    #[test]
    fn test_vector_cepea_brl_per_60kg_bag_to_usd_per_kg() {
        let brl_per_bag_e12 = parse_decimal_e12("1600").expect("valid CEPEA bag price");
        assert_eq!(brl_per_bag_e12, 1_600_000_000_000_000u128);

        let fx_brl_per_usd_e8 = parse_decimal_e8("5.00000000").expect("valid BRL/USD FX");
        assert_eq!(fx_brl_per_usd_e8, 500_000_000u128);

        let usd_per_kg_e8 =
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(brl_per_bag_e12, fx_brl_per_usd_e8)
                .expect("valid CEPEA conversion");
        assert_eq!(usd_per_kg_e8, 533_333_333u128);
        assert_eq!(format_display_price(usd_per_kg_e8, 8), "5.33333333");
    }

    #[test]
    fn test_zero_price_and_zero_fx() {
        assert_eq!(parse_decimal_e12("0").unwrap(), 0);
        assert_eq!(parse_decimal_e12("0.000000000000").unwrap(), 0);
        assert_eq!(convert_ice_usc_per_lb_e12_to_usd_per_kg_e8(0).unwrap(), 0);
        assert_eq!(
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(0, 500_000_000).unwrap(),
            0
        );

        let err = convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(1_600_000_000_000_000, 0)
            .unwrap_err();
        assert_eq!(err, FixedPointError::ZeroDenominator);

        assert_eq!(
            round_half_up_div_u128(100, 0).unwrap_err(),
            FixedPointError::ZeroDenominator
        );
    }

    #[test]
    fn test_max_representable_values_and_overflow() {
        // u128::MAX = 340_282_366_920_938_463_463_374_607_431_768_211_455
        let max_dec_e12 = "340282366920938463463374607.431768211455";
        assert_eq!(parse_decimal_e12(max_dec_e12).unwrap(), u128::MAX);

        // One unit past u128::MAX in fractional addition overflows.
        assert_eq!(
            parse_decimal_e12("340282366920938463463374607.431768211456").unwrap_err(),
            FixedPointError::Overflow
        );
        // Integer scaling overflow (`u128::MAX / 10^12 + 1`).
        assert_eq!(
            parse_decimal_e12("340282366920938463463374608").unwrap_err(),
            FixedPointError::Overflow
        );
        // Digit accumulation overflow (past u128::MAX).
        assert_eq!(
            parse_decimal_e12("999999999999999999999999999999999999999").unwrap_err(),
            FixedPointError::Overflow
        );

        // Conversion A maximum representable input before `checked_mul(100)` overflows.
        let max_ice_input = u128::MAX / 100;
        let expected_ice_num = max_ice_input * 100;
        let expected_ice_q = expected_ice_num / SI_GRAMS_PER_POUND_E8;
        let expected_ice_r = expected_ice_num % SI_GRAMS_PER_POUND_E8;
        let expected_ice = expected_ice_q
            + u128::from(expected_ice_r >= SI_GRAMS_PER_POUND_E8 - (SI_GRAMS_PER_POUND_E8 / 2));
        assert_eq!(
            convert_ice_usc_per_lb_e12_to_usd_per_kg_e8(max_ice_input).unwrap(),
            expected_ice
        );
        assert_eq!(
            convert_ice_usc_per_lb_e12_to_usd_per_kg_e8(max_ice_input + 1).unwrap_err(),
            FixedPointError::Overflow
        );
        assert_eq!(
            convert_ice_usc_per_lb_e12_to_usd_per_kg_e8(u128::MAX).unwrap_err(),
            FixedPointError::Overflow
        );

        // Conversion B maximum representable price and FX inputs before `checked_mul` overflows.
        let max_cepea_price = u128::MAX / 10_000;
        assert!(
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(max_cepea_price, 500_000_000)
                .is_ok()
        );
        assert_eq!(
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(max_cepea_price + 1, 500_000_000)
                .unwrap_err(),
            FixedPointError::Overflow
        );

        let max_fx = u128::MAX / CEPEA_BAG_WEIGHT_KG;
        assert_eq!(
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(1_000_000_000_000, max_fx).unwrap(),
            0
        );
        // `(u128::MAX / 10_000 * 10_000) / (60 * (u128::MAX / 60))` is ~0.999999..., which rounds half-up to 1.
        assert_eq!(
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(max_cepea_price, max_fx).unwrap(),
            1
        );
        assert_eq!(
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(1_000_000_000_000, max_fx + 1)
                .unwrap_err(),
            FixedPointError::Overflow
        );
    }

    #[test]
    fn test_invalid_decimal_inputs() {
        assert_eq!(
            parse_decimal_e12("").unwrap_err(),
            FixedPointError::EmptyInput
        );
        assert_eq!(
            parse_decimal_e12("   ").unwrap_err(),
            FixedPointError::EmptyInput
        );

        // Negative values
        assert_eq!(
            parse_decimal_e12("-1").unwrap_err(),
            FixedPointError::NegativeValueNotAllowed
        );
        assert_eq!(
            parse_decimal_e12("-330.12").unwrap_err(),
            FixedPointError::NegativeValueNotAllowed
        );

        // Scientific notation
        assert_eq!(
            parse_decimal_e12("1e5").unwrap_err(),
            FixedPointError::ScientificNotationNotAllowed
        );
        assert_eq!(
            parse_decimal_e12("3.3012E+2").unwrap_err(),
            FixedPointError::ScientificNotationNotAllowed
        );

        // Excessive fractional precision (>12 digits, including trailing zeros)
        assert_eq!(
            parse_decimal_e12("1.0000000000001").unwrap_err(),
            FixedPointError::ExcessiveFractionalPrecision {
                max_digits: 12,
                actual_digits: 13
            }
        );
        assert_eq!(
            parse_decimal_e12("1.0000000000000").unwrap_err(),
            FixedPointError::ExcessiveFractionalPrecision {
                max_digits: 12,
                actual_digits: 13
            }
        );
        // Exact 12 fractional digits succeeds; e8 rejects > 8 fractional digits
        assert_eq!(
            parse_decimal_e12("1.000000000001").unwrap(),
            1_000_000_000_001
        );
        assert_eq!(parse_decimal_e8("5.12345678").unwrap(), 512_345_678);
        assert_eq!(
            parse_decimal_e8("5.123456789").unwrap_err(),
            FixedPointError::ExcessiveFractionalPrecision {
                max_digits: 8,
                actual_digits: 9
            }
        );

        // Malformed syntax (spaces, separators, signs, extra/empty dots, non-digits)
        for bad in [
            " 10", "10 ", "+10.5", "1,600.00", "1_600", ".5", "5.", ".", "1.2.3", "12.34abc",
            "NaN", "inf",
        ] {
            assert!(
                matches!(
                    parse_decimal_e12(bad).unwrap_err(),
                    FixedPointError::InvalidDecimalFormat { .. }
                ),
                "expected InvalidDecimalFormat for '{}'",
                bad
            );
        }
    }

    #[test]
    fn test_rounding_half_up_boundaries() {
        // 1. Even denominator exact tie-breaking
        assert_eq!(round_half_up_div_u128(14, 10).unwrap(), 1);
        assert_eq!(round_half_up_div_u128(15, 10).unwrap(), 2); // Exact 1.5 rounds up
        assert_eq!(round_half_up_div_u128(16, 10).unwrap(), 2);

        // 2. Odd denominator boundaries (e.g., 7 -> half is 3.5)
        assert_eq!(round_half_up_div_u128(3, 7).unwrap(), 0);
        assert_eq!(round_half_up_div_u128(4, 7).unwrap(), 1);

        // 3. Large u128 numerator near u128::MAX where `(num + den/2)` would overflow
        assert_eq!(
            round_half_up_div_u128(u128::MAX, 10).unwrap(),
            u128::MAX / 10 + 1
        );
        assert_eq!(
            round_half_up_div_u128(u128::MAX - 1, u128::MAX - 1).unwrap(),
            1
        );
        assert_eq!(round_half_up_div_u128(u128::MAX, u128::MAX).unwrap(), 1);

        // 4. CEPEA exact half-unit boundaries at FX = 5.00000000 (den = 30_000_000_000)
        // Half remainder threshold = 15_000_000_000 -> brl_per_bag_e12 delta = 1_500_000
        let fx = 500_000_000u128;
        let base_brl = 1_500_000_000_000_000u128; // 1500 BRL/bag -> exact 500_000_000 ($5.00/kg)
        assert_eq!(
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(base_brl + 1_499_999, fx).unwrap(),
            500_000_000
        );
        assert_eq!(
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(base_brl + 1_500_000, fx).unwrap(),
            500_000_001
        );
        assert_eq!(
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(base_brl + 1_500_001, fx).unwrap(),
            500_000_001
        );

        // 5. ICE Coffee C closest integer half-unit boundaries around odd denominator 45_359_237
        // Since 12_246_994 * 100 ≡ 1 (mod 45_359_237), modular remainder moves by exactly 1 per step:
        // half - 0.5 = 22_679_618 (rounds down) vs half + 0.5 = 22_679_619 (rounds up).
        const INV_100_MOD_SI: u128 = 12_246_994;
        let x_just_below = (22_679_618u128 * INV_100_MOD_SI) % SI_GRAMS_PER_POUND_E8;
        let x_just_above = (22_679_619u128 * INV_100_MOD_SI) % SI_GRAMS_PER_POUND_E8;
        assert_eq!((x_just_below * 100) % SI_GRAMS_PER_POUND_E8, 22_679_618);
        assert_eq!((x_just_above * 100) % SI_GRAMS_PER_POUND_E8, 22_679_619);
        assert_eq!(
            convert_ice_usc_per_lb_e12_to_usd_per_kg_e8(x_just_below).unwrap(),
            (x_just_below * 100) / SI_GRAMS_PER_POUND_E8
        );
        assert_eq!(
            convert_ice_usc_per_lb_e12_to_usd_per_kg_e8(x_just_above).unwrap(),
            ((x_just_above * 100) / SI_GRAMS_PER_POUND_E8) + 1
        );
    }

    #[test]
    fn test_cepea_fx_direction_not_inverted() {
        // Higher FX (more BRL per 1 USD, weaker BRL) must yield a LOWER USD/kg price.
        let bag_price = parse_decimal_e12("1600").unwrap();
        let usd_at_fx_4 =
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(bag_price, 400_000_000).unwrap();
        let usd_at_fx_8 =
            convert_cepea_brl_per_60kg_bag_e12_to_usd_per_kg_e8(bag_price, 800_000_000).unwrap();
        assert!(usd_at_fx_4 > usd_at_fx_8);
        assert_eq!(usd_at_fx_4, 666_666_667); // 1600 / 60 / 4 = 6.666666666... -> 666_666_667
        assert_eq!(usd_at_fx_8, 333_333_333); // 1600 / 60 / 8 = 3.333333333... -> 333_333_333
    }

    #[test]
    fn test_imf_pcps_high_precision_round_half_up_and_usd_per_kg_conversion() {
        // Exact observed IMF PCPS PCOFFOTM string (`2026-M09`: 13 fractional digits)
        // and FRED mirror string (`2026-09-01`: 14 fractional digits).
        let imf_e12 = parse_decimal_e12_round_half_up("330.1219047619048")
            .expect("13-digit IMF PCPS decimal must parse with round-half-up");
        assert_eq!(imf_e12, 330_121_904_761_905u128);

        let fred_e12 = parse_decimal_e12_round_half_up("330.12190476190477")
            .expect("14-digit decimal must also round half-up deterministically");
        assert_eq!(fred_e12, 330_121_904_761_905u128);

        let usd_per_kg_e8 =
            convert_usc_per_lb_e12_to_usd_per_kg_e8(imf_e12).expect("valid USD/kg conversion");
        assert_eq!(usd_per_kg_e8, 727_794_219u128);
        assert_eq!(format_display_price(usd_per_kg_e8, 8), "7.27794219");

        // Exact half-up rounding ties at the 13th fractional digit:
        assert_eq!(
            parse_decimal_e12_round_half_up("1.000000000000499999999999").unwrap(),
            1_000_000_000_000u128
        );
        assert_eq!(
            parse_decimal_e12_round_half_up("1.0000000000005").unwrap(),
            1_000_000_000_001u128
        );
        assert_eq!(
            parse_decimal_e12_round_half_up("1.000000000000500000000000").unwrap(),
            1_000_000_000_001u128
        );
        // Ripple carry across all twelve fractional nines into whole unit
        assert_eq!(
            parse_decimal_e12_round_half_up("9.9999999999995").unwrap(),
            10_000_000_000_000u128
        );
        // Up to 24 fractional digits are supported; <= 12 digits match `parse_decimal_e12`
        assert_eq!(
            parse_decimal_e12_round_half_up("267.65").unwrap(),
            parse_decimal_e12("267.65").unwrap()
        );
        assert_eq!(
            convert_usc_per_lb_e12_to_usd_per_kg_e8(
                parse_decimal_e12_round_half_up("267.65").unwrap()
            )
            .unwrap(),
            590_067_245u128
        );
    }

    #[test]
    fn test_high_precision_decimal_rejects_excessive_digits_and_round_up_overflow() {
        // 25 fractional digits exceeds MAX_HIGH_PRECISION_DECIMAL_FRAC_DIGITS (24)
        let frac_25 = format!("1.{}", "0".repeat(25));
        assert_eq!(
            parse_decimal_e12_round_half_up(&frac_25).unwrap_err(),
            FixedPointError::ExcessiveFractionalPrecision {
                max_digits: MAX_HIGH_PRECISION_DECIMAL_FRAC_DIGITS as u8,
                actual_digits: 25
            }
        );

        // Rounding half-up at u128::MAX fails closed with Overflow
        assert_eq!(
            parse_decimal_e12_round_half_up("340282366920938463463374607.4317682114555")
                .unwrap_err(),
            FixedPointError::Overflow
        );
        // Below 0.5 at u128::MAX stays u128::MAX
        assert_eq!(
            parse_decimal_e12_round_half_up("340282366920938463463374607.4317682114554").unwrap(),
            u128::MAX
        );

        // Strict parse_decimal_e12 still rejects 13 fractional digits
        assert_eq!(
            parse_decimal_e12("330.1219047619048").unwrap_err(),
            FixedPointError::ExcessiveFractionalPrecision {
                max_digits: 12,
                actual_digits: 13
            }
        );

        // Reject negatives, scientific notation, empty, malformed strings in high-precision parser too
        assert_eq!(
            parse_decimal_e12_round_half_up("-330.1219047619048").unwrap_err(),
            FixedPointError::NegativeValueNotAllowed
        );
        assert_eq!(
            parse_decimal_e12_round_half_up("3.301219047619048e2").unwrap_err(),
            FixedPointError::ScientificNotationNotAllowed
        );
        assert_eq!(
            parse_decimal_e12_round_half_up("").unwrap_err(),
            FixedPointError::EmptyInput
        );
    }
}
