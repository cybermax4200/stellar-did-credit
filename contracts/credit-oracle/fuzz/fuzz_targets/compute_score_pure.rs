#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 44 {
        return;
    }

    let vc_points = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let volume_30d = i128::from_le_bytes([
        data[4], data[5], data[6], data[7], data[8], data[9], data[10], data[11],
        data[12], data[13], data[14], data[15], data[16], data[17], data[18], data[19],
    ]);
    let avg_counterparties = u32::from_le_bytes([data[20], data[21], data[22], data[23]]);
    let on_time_count = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
    let total_count = u32::from_le_bytes([data[28], data[29], data[30], data[31]]);
    let vc_weight = u32::from_le_bytes([data[32], data[33], data[34], data[35]]);
    let tx_weight = u32::from_le_bytes([data[36], data[37], data[38], data[39]]);
    let repayment_weight = u32::from_le_bytes([data[40], data[41], data[42], data[43]]);

    let score = credit_oracle::compute_score_pure(
        vc_points,
        volume_30d,
        avg_counterparties,
        on_time_count,
        total_count,
        vc_weight,
        tx_weight,
        repayment_weight,
    );

    assert!(
        score >= 300,
        "score {} is below MIN_SCORE 300: vc_points={} volume_30d={} counterparties={} on_time={} total={} vc_w={} tx_w={} repay_w={}",
        score, vc_points, volume_30d, avg_counterparties, on_time_count, total_count,
        vc_weight, tx_weight, repayment_weight,
    );
    assert!(
        score <= 850,
        "score {} is above MAX_SCORE 850: vc_points={} volume_30d={} counterparties={} on_time={} total={} vc_w={} tx_w={} repay_w={}",
        score, vc_points, volume_30d, avg_counterparties, on_time_count, total_count,
        vc_weight, tx_weight, repayment_weight,
    );
});
