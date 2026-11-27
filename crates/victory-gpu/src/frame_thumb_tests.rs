use super::*;
use windows::Win32::{Foundation::RECT, Graphics::Dxgi::Common::*};
#[test]
fn warp_box_and_file() {
    if std::env::var("VICTORY_D3D12_WARP").as_deref() != Ok("1") {
        eprintln!("skipping WARP-only frame thumbnail test");
        return;
    }
    struct WarnEvents;
    impl tracing::Subscriber for WarnEvents {
        fn enabled(&self, m: &tracing::Metadata<'_>) -> bool {
            m.target().ends_with("frame_thumb") && *m.level() == tracing::Level::WARN
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::Id, _: &tracing::Id) {}
        fn enter(&self, _: &tracing::Id) {}
        fn exit(&self, _: &tracing::Id) {}
        fn event(&self, e: &tracing::Event<'_>) {
            e.record(&mut |f: &tracing::field::Field, v: &dyn std::fmt::Debug| {
                if f.name() == "message" {
                    eprintln!("WARN {v:?}")
                }
            });
        }
    }
    let _logs = tracing::subscriber::set_default(WarnEvents);
    let host = crate::host::host().expect("WARP device");
    assert!(Thumb::configured(host, false, || panic!("off initialized resources")).is_none());
    let path = std::env::temp_dir().join(format!("victory-thumb-test-{}.bin", std::process::id()));
    let mut t = Thumb::configured(host, true, || path.clone()).unwrap();
    let target = D3D12_RESOURCE_STATE_RENDER_TARGET;
    unsafe {
        let mut tex: Option<ID3D12Resource> = None;
        host.device
            .CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: 193,
                    Height: 123,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                    ..Default::default()
                },
                target,
                None,
                &mut tex,
            )
            .unwrap();
        let tex = tex.unwrap();
        let queue: ID3D12CommandQueue = host
            .device
            .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default())
            .unwrap();
        // Delay only the test completion signals. All four descriptors and
        // readback slots must remain owned, even though host.execute returns.
        let gate: ID3D12Fence = host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        queue.Wait(&gate, 1).unwrap();
        for n in 0..4 {
            let list = host.new_list().unwrap();
            let cmd = &list.list;
            let rtv = host.rtv(Some(&tex), None);
            cmd.ClearRenderTargetView(rtv, &[1., 0., 0., 1.], None);
            cmd.ClearRenderTargetView(
                rtv,
                &[0., 1., 0., 1.],
                Some(&[RECT {
                    left: 71 + n,
                    top: 111,
                    right: 193,
                    bottom: 123,
                }]),
            );
            cmd.ResourceBarrier(&[transition(&tex, target, D3D12_RESOURCE_STATE_COPY_DEST)]);
            let i = t.record(host, cmd, &tex).unwrap();
            cmd.ResourceBarrier(&[transition(&tex, D3D12_RESOURCE_STATE_COPY_DEST, target)]);
            list.close();
            host.execute(&[&list]);
            let ready = t.signal(&queue, Some(i));
            assert!(ready.is_some());
            t.at_present(71 + n as u64, ready, Thumb::clock());
        }
        host.device.GetDeviceRemovedReason().unwrap();
        let fifth = host.new_list().unwrap();
        let full = t.record(host, &fifth.list, &tex);
        fifth.close();
        gate.Signal(1).unwrap();
        assert!(full.is_none(), "overwrote a pending thumbnail slot");
        let skip = t.signal(&queue, full);
        t.at_present(75, skip, Thumb::clock());
    }
    assert_eq!(
        Stamp {
            frame: 74,
            present: 4,
            qpc: 100
        }
        .line(None),
        "frame-thumb frame=74 present=4 qpc=100 off=missing"
    );
    let start = Instant::now();
    let bytes = loop {
        if let Ok(b) = std::fs::read(&path) {
            if b.len() == 32 + 4 * SIZE {
                break b;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "worker did not flush valid records"
        );
        std::thread::sleep(Duration::from_millis(10))
    };
    assert_eq!(&bytes[..20], b"VTHUMB01\x40\0\x24\0\0\x01\x18\0\0\x21\0\0");
    assert_eq!(&bytes[20..28], &header()[20..28]);
    assert_eq!(&bytes[28..32], &[0; 4]);
    for n in 0..4 {
        let mut pos = 32 + n * SIZE;
        for (cols, rows, y0) in [(64, 36, 0.), (256, 24, 98.4)] {
            for y in 0..rows {
                for x in 0..cols {
                    let dx = 193. / cols as f64;
                    let dy = (123. - y0) / rows as f64;
                    let left = x as f64 * dx;
                    let top = y0 + y as f64 * dy;
                    let green = ((left + dx - (71. + n as f64)).clamp(0., dx) / dx)
                        * ((top + dy - 111.).clamp(0., dy) / dy);
                    let want = ((0.2126 * (1. - green) + 0.7152 * green) * 255.).round() as u8;
                    assert!(
                        bytes[pos].abs_diff(want) <= 1,
                        "record {n} cell {x},{y} byte {pos}: {} expected {want}",
                        bytes[pos]
                    );
                    pos += 1;
                }
            }
        }
    }
    eprintln!(
        "WARP: 4 records x 8448 gray bytes match CPU box/Rec.709 (+/-1); 32-byte header and missing format verified"
    );
}
