use std::io::Cursor;

use criterion::{Criterion, criterion_group, criterion_main};
use kazoo_core::ipc::protocol::{
    AUDIO_PAYLOAD_HEADER, FrameBuffer, HEADER_SIZE, decode_audio_samples, encode_audio_payload,
};
use kazoo_core::ipc::types::MSG_AUDIO;

fn link_path(c: &mut Criterion) {
    let samples: Vec<f32> = (0..512)
        .flat_map(|frame| {
            let sample = (std::f32::consts::TAU * 110.0 * frame as f32 / 48_000.0).sin()
                * 10.0_f32.powf(-12.0 / 20.0);
            [sample, sample]
        })
        .collect();
    let payload_size = AUDIO_PAYLOAD_HEADER + samples.len() * size_of::<f32>();
    let mut payload = vec![0; payload_size];
    let mut frame = FrameBuffer::new();
    let mut writer = Cursor::new(Vec::with_capacity(HEADER_SIZE + payload_size));
    c.bench_function("link/encode/512", |b| {
        b.iter(|| {
            let size = encode_audio_payload(0, 512, &samples, &mut payload);
            frame.payload_mut()[..size].copy_from_slice(&payload[..size]);
            writer.set_position(0);
            frame.write_frame(MSG_AUDIO, 0, size, &mut writer).unwrap();
            std::hint::black_box(writer.get_ref());
        });
    });
    let size = encode_audio_payload(0, 512, &samples, &mut payload);
    frame.payload_mut()[..size].copy_from_slice(&payload[..size]);
    writer.set_position(0);
    frame.write_frame(MSG_AUDIO, 0, size, &mut writer).unwrap();
    let bytes = writer.into_inner();
    let mut decoded = vec![0.0; samples.len()];
    let mut reader = Cursor::new(bytes.as_slice());
    c.bench_function("link/decode/512", |b| {
        b.iter(|| {
            reader.set_position(0);
            let header = frame
                .try_read_frame(&mut reader)
                .unwrap()
                .expect("complete frame");
            assert_eq!(header.payload_len as usize, payload_size);
            decode_audio_samples(
                &frame.payload()[..payload_size],
                &mut decoded,
                samples.len(),
            );
            std::hint::black_box(&decoded);
        });
    });
}

criterion_group!(benches, link_path);
criterion_main!(benches);
