//! A panic in a system ends `start` with `ECS_RUNNER_PANICKED` instead of
//! panicking the thread that awaits it.

use std::{num::NonZero, time::Duration};

use azalea::{
    app::{App, Update},
    ecs::prelude::Local,
    prelude::*,
    swarm::ECS_RUNNER_PANICKED,
};

fn panicking_plugin(app: &mut App) {
    // `start` runs the schedule once itself before the runner starts
    app.add_systems(Update, |mut runs: Local<u32>| {
        *runs += 1;
        if *runs == 3 {
            panic!("injected system panic");
        }
    });
}

#[test]
fn ecs_runner_panic_returns_an_exit_code() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let exit = rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(30),
            ClientBuilder::new()
                .add_plugins(panicking_plugin)
                .reconnect_after(None)
                // nothing listens there; the panic comes first anyway
                .start(Account::offline("bot"), "127.0.0.1:9"),
        )
        .await
        .expect("start returned")
    });
    assert_eq!(
        exit,
        AppExit::Error(NonZero::new(ECS_RUNNER_PANICKED).unwrap())
    );
}
