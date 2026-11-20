use std::{hint::black_box, io::Cursor};

use azalea_buf::AzBuf;
use azalea_core::{delta::QuantizedVelocity, entity_id::MinecraftEntityId, position::Vec3};
use azalea_protocol::packets::game::ClientboundAddEntity;
use azalea_registry::builtin::EntityKind;
use criterion::{Criterion, criterion_group, criterion_main};
use uuid::Uuid;

fn benchmark(c: &mut Criterion) {
    c.bench_function("c_add_entity", |b| {
        let mut buf = Vec::new();
        ClientboundAddEntity {
            id: MinecraftEntityId(1),
            uuid: Uuid::nil(),
            entity_type: EntityKind::Zombie,
            position: Vec3::new(1., 67., 0.),
            x_rot: 0,
            y_rot: 0,
            y_head_rot: 0,
            data: 0,
            movement: QuantizedVelocity::from_vec3(Vec3::new(0., -0.08, 0.)),
        }
        .azalea_write(&mut buf)
        .unwrap();
        b.iter(|| {
            black_box(ClientboundAddEntity::azalea_read(&mut Cursor::new(&buf)).unwrap());
        });
    });
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
