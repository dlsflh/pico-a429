use arinc429_core::protocol::{
    BINARY_FRAME_LEN, BinaryFrame, find_frame_in_stream, frame_types, opcodes,
};

#[test]
fn test_stream_tx_data_frame_layout_and_codec() {
    let channel = 3u8;
    let seq_num = 1234u16;
    let word = 0xE048D013u32;
    let tag = 9999u32;
    let frame = BinaryFrame::new_stream_tx(channel, seq_num, word, tag, true);

    assert_eq!(frame.frame_type, frame_types::STREAM_TX_DATA);
    assert_eq!(frame.channel, channel);
    assert_eq!(frame.flags, 0x01); // odd_parity enable
    assert_eq!(frame.seq_num, seq_num);
    assert_eq!(frame.stream_word(), word);
    assert_eq!(frame.payload_1, tag);

    let mut buf = [0u8; BINARY_FRAME_LEN * 2];
    let encoded_len = frame.encode_to_slice(&mut buf[..BINARY_FRAME_LEN]);
    assert_eq!(encoded_len, 16);

    let (decoded, consumed) =
        find_frame_in_stream(&buf[..BINARY_FRAME_LEN]).expect("Should decode cleanly");
    assert_eq!(consumed, 16);
    assert_eq!(decoded.frame_type, frame_types::STREAM_TX_DATA);
    assert_eq!(decoded.channel, channel);
    assert_eq!(decoded.flags, 0x01);
    assert_eq!(decoded.seq_num, seq_num);
    assert_eq!(decoded.stream_word(), word);
    assert_eq!(decoded.payload_1, tag);
}

#[test]
fn test_stream_credit_frame_layout_and_codec() {
    let channel = 5u8;
    let seq_num = 4321u16;
    let available_slots = 96u32;
    let cumulative_handled = 100_000u32;
    let frame =
        BinaryFrame::new_stream_credit(channel, seq_num, available_slots, cumulative_handled);

    assert_eq!(frame.frame_type, frame_types::STREAM_CREDIT);
    assert_eq!(frame.channel, channel);
    assert_eq!(frame.flags, 0x00);
    assert_eq!(frame.seq_num, seq_num);
    assert_eq!(frame.credit_available_slots(), available_slots);
    assert_eq!(frame.credit_cumulative_handled(), cumulative_handled);

    let mut buf = [0u8; BINARY_FRAME_LEN];
    frame.encode_to_slice(&mut buf);

    let (decoded, consumed) = find_frame_in_stream(&buf).expect("Should decode stream credit");
    assert_eq!(consumed, 16);
    assert_eq!(decoded.frame_type, frame_types::STREAM_CREDIT);
    assert_eq!(decoded.channel, channel);
    assert_eq!(decoded.credit_available_slots(), available_slots);
    assert_eq!(decoded.credit_cumulative_handled(), cumulative_handled);
}

#[test]
fn test_abort_tx_cmd_req() {
    let channel = 2u8;
    let seq = 55u16;
    let frame = BinaryFrame::new_cmd_abort_tx(channel, seq);

    assert_eq!(frame.frame_type, frame_types::CMD_REQ);
    assert_eq!(frame.channel, channel);
    assert_eq!(frame.opcode(), opcodes::ABORT_TX);
    assert_eq!(frame.seq_num, seq);

    let mut buf = [0u8; BINARY_FRAME_LEN];
    frame.encode_to_slice(&mut buf);

    let (decoded, consumed) = find_frame_in_stream(&buf).expect("Should decode abort tx cmd");
    assert_eq!(consumed, 16);
    assert_eq!(decoded.opcode(), opcodes::ABORT_TX);
}

#[test]
fn test_stream_frame_crc_corruption_rejection() {
    let frame = BinaryFrame::new_stream_tx(0, 1, 0x12345678, 0, false);
    let mut buf = [0u8; BINARY_FRAME_LEN];
    frame.encode_to_slice(&mut buf);

    // Corrupt one bit in payload
    buf[7] ^= 0x04;
    assert!(find_frame_in_stream(&buf).is_none());
}
