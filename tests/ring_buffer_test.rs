use webtop::collector::snapshot::SystemSnapshot;
use webtop::storage::ring_buffer::RingBuffer;

#[test]
fn push_and_retrieve() {
    let mut rb = RingBuffer::new(3);
    let s1 = SystemSnapshot {
        timestamp: 1,
        ..Default::default()
    };
    let s2 = SystemSnapshot {
        timestamp: 2,
        ..Default::default()
    };
    rb.push(s1);
    rb.push(s2);
    let all = rb.get_all();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].timestamp, 1);
    assert_eq!(all[1].timestamp, 2);
}

#[test]
fn evicts_oldest_when_full() {
    let mut rb = RingBuffer::new(2);
    rb.push(SystemSnapshot {
        timestamp: 1,
        ..Default::default()
    });
    rb.push(SystemSnapshot {
        timestamp: 2,
        ..Default::default()
    });
    rb.push(SystemSnapshot {
        timestamp: 3,
        ..Default::default()
    });
    let all = rb.get_all();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].timestamp, 2);
    assert_eq!(all[1].timestamp, 3);
}

#[test]
fn latest_returns_last() {
    let mut rb = RingBuffer::new(10);
    rb.push(SystemSnapshot {
        timestamp: 100,
        ..Default::default()
    });
    rb.push(SystemSnapshot {
        timestamp: 200,
        ..Default::default()
    });
    assert_eq!(rb.latest().unwrap().timestamp, 200);
}

#[test]
fn get_since_filters_by_timestamp() {
    let mut rb = RingBuffer::new(10);
    for i in 0..5 {
        rb.push(SystemSnapshot {
            timestamp: i * 1000,
            ..Default::default()
        });
    }
    let since_2000 = rb.get_since(2000);
    assert_eq!(since_2000.len(), 3);
}
