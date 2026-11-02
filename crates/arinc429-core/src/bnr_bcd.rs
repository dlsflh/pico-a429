//! ARINC 429 BNR (二进制数值) 与 BCD (二-十进制) 解析辅助
//!
//! - BNR: 数据字段通常为补码有符号数，最高位（Bit 29）为符号位。
//! - BCD: 4 位一组表示十进制数字 0~9。

/// 解码 19 位 BNR 字段为有符号整数 (i32)
///
/// `bits_used`: 实际使用的有效数据位数 (包括符号位，通常在 12 ~ 19 之间)
#[inline]
pub fn decode_bnr_to_int(data_19bit: u32, bits_used: u8) -> i32 {
    let bits_used = bits_used.min(19).max(1);
    // 数据字段为 19 位，右对齐或左对齐通常取决于具体航电参数规范。
    // 在 ARINC 429 中，Bit 29 为 MSB (符号位)，即 19 位中的最高位 bit 18。
    let shift = 19 - bits_used;
    let aligned = (data_19bit >> shift) & ((1 << bits_used) - 1);

    // 符号扩展
    let sign_bit = 1 << (bits_used - 1);
    if (aligned & sign_bit) != 0 {
        // 负数：将高位全部置 1
        (aligned | (!((1 << bits_used) - 1))) as i32
    } else {
        aligned as i32
    }
}

/// 将工程物理量编码为 19 位 BNR 字段
#[inline]
pub fn encode_bnr_from_int(value: i32, bits_used: u8) -> u32 {
    let bits_used = bits_used.min(19).max(1);
    let mask = (1 << bits_used) - 1;
    let raw = (value as u32) & mask;
    let shift = 19 - bits_used;
    (raw << shift) & 0x0007_FFFF
}

/// 解码 BCD 字段为 5 位数字数组 (每位 0~9，若为 0b1111 或无效则返回 None)
pub fn decode_bcd_digits(data_19bit: u32) -> [Option<u8>; 5] {
    // 19 位分为：Digit 5 (高 3 位，Bit 27~29), Digit 4 (4位), Digit 3 (4位), Digit 2 (4位), Digit 1 (4位)
    let mut digits = [None; 5];

    // D1: Bits 11~14 -> data[0..4]
    let d1 = (data_19bit & 0x0F) as u8;
    if d1 <= 9 { digits[0] = Some(d1); }

    // D2: Bits 15~18 -> data[4..8]
    let d2 = ((data_19bit >> 4) & 0x0F) as u8;
    if d2 <= 9 { digits[1] = Some(d2); }

    // D3: Bits 19~22 -> data[8..12]
    let d3 = ((data_19bit >> 8) & 0x0F) as u8;
    if d3 <= 9 { digits[2] = Some(d3); }

    // D4: Bits 23~26 -> data[12..16]
    let d4 = ((data_19bit >> 12) & 0x0F) as u8;
    if d4 <= 9 { digits[3] = Some(d4); }

    // D5: Bits 27~29 -> data[16..19] (最高 3 位，0~7)
    let d5 = ((data_19bit >> 16) & 0x07) as u8;
    digits[4] = Some(d5);

    digits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bnr_codec() {
        // 16 位 BNR 测试
        let val = 1234;
        let encoded = encode_bnr_from_int(val, 16);
        let decoded = decode_bnr_to_int(encoded, 16);
        assert_eq!(decoded, val);

        // 负数测试
        let val_neg = -5678;
        let encoded_neg = encode_bnr_from_int(val_neg, 16);
        let decoded_neg = decode_bnr_to_int(encoded_neg, 16);
        assert_eq!(decoded_neg, val_neg);
    }

    #[test]
    fn test_bcd_digits() {
        // 构造 BCD: 1, 2, 3, 4, 5
        let d1 = 1;
        let d2 = 2;
        let d3 = 3;
        let d4 = 4;
        let d5 = 5;
        let bcd_data = d1 | (d2 << 4) | (d3 << 8) | (d4 << 12) | (d5 << 16);

        let digits = decode_bcd_digits(bcd_data);
        assert_eq!(digits[0], Some(1));
        assert_eq!(digits[1], Some(2));
        assert_eq!(digits[2], Some(3));
        assert_eq!(digits[3], Some(4));
        assert_eq!(digits[4], Some(5));
    }
}
