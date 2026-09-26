use std::time::Duration;

use paco_runtime::{Runtime, channel, spawn};

#[test]
fn receiver_resumes_exactly_when_a_value_is_sent() {
    let runtime = Runtime::new(2);
    let (tx, rx) = channel::<i32>(&runtime, 1);

    let receiver = spawn(&runtime, move || rx.recv().unwrap());

    std::thread::sleep(Duration::from_millis(20));
    tx.send(42).unwrap();

    assert_eq!(receiver.join().unwrap(), 42);
}

#[test]
fn sender_blocks_on_a_full_channel_until_receiver_makes_room() {
    let runtime = Runtime::new(2);
    let (tx, rx) = channel::<i32>(&runtime, 1);

    tx.send(1).unwrap();
    let tx2 = tx.clone();
    let sender = spawn(&runtime, move || tx2.send(2).unwrap());

    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(rx.recv().unwrap(), 1);
    sender.join().unwrap();
    assert_eq!(rx.recv().unwrap(), 2);
}

#[test]
fn sender_close_closes_the_channel_even_with_clones_still_alive() {
    let runtime = Runtime::new(1);
    let (tx, rx) = channel::<i32>(&runtime, 1);
    let _tx2 = tx.clone();
    tx.close();
    assert!(rx.recv().is_err());
    assert!(tx.send(1).is_err());
}

#[test]
fn recv_on_a_closed_empty_channel_errors() {
    let runtime = Runtime::new(1);
    let (tx, rx) = channel::<i32>(&runtime, 1);
    drop(tx);
    assert!(rx.recv().is_err());
}

#[test]
fn producer_consumer_across_many_tasks() {
    let runtime = Runtime::new(4);
    let (tx, rx) = channel::<i32>(&runtime, 4);

    let producers: Vec<_> = (0..10)
        .map(|i| {
            let tx = tx.clone();
            spawn(&runtime, move || tx.send(i).unwrap())
        })
        .collect();
    drop(tx);

    let consumer = spawn(&runtime, move || {
        let mut sum = 0;
        while let Ok(value) = rx.recv() {
            sum += value;
        }
        sum
    });

    for producer in producers {
        producer.join().unwrap();
    }
    assert_eq!(consumer.join().unwrap(), 45);
}

#[test]
fn a_thread_receiver_never_misses_the_final_close_wakeup() {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = Runtime::new(2);
        for _ in 0..2000 {
            let (tx, rx) = channel::<i32>(&runtime, 8);
            let producer = spawn(&runtime, move || {
                for i in 0..5 {
                    tx.send(i).unwrap();
                }
                tx.close();
            });
            let mut sum = 0;
            while let Ok(value) = rx.recv() {
                sum += value;
            }
            producer.join().unwrap();
            assert_eq!(sum, 10);
        }
        done_tx.send(()).unwrap();
    });
    done_rx
        .recv_timeout(Duration::from_secs(60))
        .expect("a receiver parked after the producer's last wakeup and never woke");
}
