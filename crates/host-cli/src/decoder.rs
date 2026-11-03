//! 上位机流式分帧解码器 (Stream Frame Decoder)
//!
//! 负责从任意分片或粘包的字节流中重对齐、校验并提取定长 14 字节的 ARINC 429 二进制帧。

use arinc429_core::protocol::{BinaryFrame, BINARY_FRAME_LEN, BINARY_MAGIC};

pub struct FrameStreamDecoder {
    buffer: [u8; 2048],
    valid_len: usize,
}

impl Default for FrameStreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameStreamDecoder {
    pub const fn new() -> Self {
        Self {
            buffer: [0u8; 2048],
            valid_len: 0,
        }
    }

    /// 喂入新的字节切片并返回当前已完整解码的帧列表
    pub fn feed(&mut self, data: &[u8]) -> Vec<BinaryFrame> {
        let mut frames = Vec::new();

        // 溢出防护：若即将溢出，先压缩已有游标，仍溢出则重置
        if self.valid_len + data.len() > self.buffer.len() {
            self.valid_len = 0;
        }

        self.buffer[self.valid_len..self.valid_len + data.len()].copy_from_slice(data);
        self.valid_len += data.len();

        let mut cursor = 0;
        while cursor + BINARY_FRAME_LEN <= self.valid_len {
            // 搜索 Magic 头部 0x41, 0x29 ("A)")
            if self.buffer[cursor] == BINARY_MAGIC[0] && self.buffer[cursor + 1] == BINARY_MAGIC[1] {
                let candidate = &self.buffer[cursor..cursor + BINARY_FRAME_LEN];
                if let Ok(frame) = BinaryFrame::decode(candidate) {
                    frames.push(frame);
                    cursor += BINARY_FRAME_LEN;
                    continue;
                }
            }
            cursor += 1;
        }

        // 压缩未处理的尾部残留数据
        if cursor > 0 {
            self.buffer.copy_within(cursor..self.valid_len, 0);
            self.valid_len -= cursor;
        }

        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decoder_single_frame() {
        let mut decoder = FrameStreamDecoder::new();
        let frame = BinaryFrame::new(0, 0, 1000, 0x8123_45C8);
        let mut encoded = [0u8; BINARY_FRAME_LEN];
        frame.encode(&mut encoded);

        let decoded = decoder.feed(&encoded);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0], frame);
    }

    #[test]
    fn test_decoder_chunked_arrival() {
        let mut decoder = FrameStreamDecoder::new();
        let frame = BinaryFrame::new(1, 0, 2000, 0x1234_5678);
        let mut encoded = [0u8; BINARY_FRAME_LEN];
        frame.encode(&mut encoded);

        // 前 6 字节到达
        let res1 = decoder.feed(&encoded[..6]);
        assert_eq!(res1.len(), 0);

        // 后 10 字节到达
        let res2 = decoder.feed(&encoded[6..]);
        assert_eq!(res2.len(), 1);
        assert_eq!(res2[0], frame);
    }

    #[test]
    fn test_decoder_noisy_stream_recovery() {
        let mut decoder = FrameStreamDecoder::new();
        let frame = BinaryFrame::new(2, 0, 3000, 0xAABB_CCDD);
        let mut encoded = [0u8; BINARY_FRAME_LEN];
        frame.encode(&mut encoded);

        // 前置噪声数据
        let noise = [0xFF, 0x00, 0x41, 0x55, 0x12];
        let mut stream = Vec::new();
        stream.extend_from_slice(&noise);
        stream.extend_from_slice(&encoded);

        let res = decoder.feed(&stream);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0], frame);
    }

    #[test]
    fn test_decoder_back_to_back_frames() {
        let mut decoder = FrameStreamDecoder::new();
        let frame1 = BinaryFrame::new(0, 0, 100, 0x1111_1111);
        let frame2 = BinaryFrame::new(1, 0, 200, 0x2222_2222);

        let mut buf = [0u8; BINARY_FRAME_LEN * 2];
        let mut enc1 = [0u8; BINARY_FRAME_LEN];
        let mut enc2 = [0u8; BINARY_FRAME_LEN];
        frame1.encode(&mut enc1);
        frame2.encode(&mut enc2);

        buf[..BINARY_FRAME_LEN].copy_from_slice(&enc1);
        buf[BINARY_FRAME_LEN..].copy_from_slice(&enc2);

        let decoded = decoder.feed(&buf);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0], frame1);
        assert_eq!(decoded[1], frame2);
    }
}
