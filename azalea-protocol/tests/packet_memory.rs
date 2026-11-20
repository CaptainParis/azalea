//! The per-packet memory budget (`azalea_buf::limits::PACKET_MEMORY_BUDGET`,
//! 64 MiB): a packet's collections can't hold more than that in memory, even
//! when their elements are tiny on the wire. An 8 MiB `container_set_content`
//! of empty item stacks (1 byte each on the wire, `size_of::<ItemStack>()` in
//! memory) is rejected before its items are allocated, so decoding it never
//! has more than the budget allocated (measured by a counting allocator).

use std::{
    alloc::{GlobalAlloc, Layout, System},
    io::Cursor,
    sync::atomic::{AtomicUsize, Ordering},
};

use azalea_buf::{
    AzBufVar,
    limits::{LimitKind, PACKET_MEMORY_BUDGET},
};
use azalea_core::decode_errors::decode_errors;
use azalea_inventory::ItemStack;
use azalea_protocol::{
    packets::{
        ProtocolPacket,
        game::{ClientboundContainerSetContent, ClientboundGamePacket},
    },
    read::{ReadPacketError, deserialize_packet},
};

struct Counting;
static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = NOW.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(now, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        NOW.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn set_content_of_empty_stacks(n: usize) -> Vec<u8> {
    let id = ClientboundGamePacket::ContainerSetContent(ClientboundContainerSetContent {
        container_id: 0,
        state_id: 0,
        items: vec![],
        carried_item: ItemStack::Empty,
    })
    .id();
    let mut raw = Vec::with_capacity(n + 16);
    id.azalea_write_var(&mut raw).unwrap();
    0i32.azalea_write_var(&mut raw).unwrap();
    0u32.azalea_write_var(&mut raw).unwrap();
    (n as i32).azalea_write_var(&mut raw).unwrap();
    // an empty stack is a count of 0
    raw.resize(raw.len() + n, 0);
    // the carried item
    raw.push(0);
    raw
}

#[test]
fn eight_mib_of_empty_item_stacks_stays_under_the_budget() {
    // the case is real: an empty stack is much bigger in memory than on the
    // wire
    assert!(size_of::<ItemStack>() > 8, "{}", size_of::<ItemStack>());

    // 8 MiB minus the header: the largest decompressed packet
    let raw = set_content_of_empty_stacks((8 << 20) - 16);
    let before = decode_errors();
    let base = NOW.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);

    let res = deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(&raw[..]));

    let peak = PEAK.load(Ordering::Relaxed) - base;
    eprintln!(
        "size_of::<ItemStack>() = {}, peak while decoding: {peak} bytes",
        size_of::<ItemStack>()
    );
    assert!(peak < PACKET_MEMORY_BUDGET as usize, "{peak}");
    let err = res.expect_err("over the budget");
    let ReadPacketError::Parse { source, .. } = &*err else {
        panic!("{err:?}");
    };
    assert!(
        matches!(
            source,
            azalea_buf::BufReadError::Limit {
                kind: LimitKind::PacketMemory,
                ..
            }
        ),
        "{source:?}"
    );
    assert_eq!(decode_errors().packet_memory, before.packet_memory + 1);
}

#[test]
fn a_real_sized_container_is_under_the_budget() {
    // a double chest and the player's inventory: 90 slots
    let raw = set_content_of_empty_stacks(90);
    let p = deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(&raw[..])).unwrap();
    let ClientboundGamePacket::ContainerSetContent(p) = p else {
        panic!()
    };
    assert_eq!(p.items.len(), 90);
}

#[test]
fn budget_boundary() {
    // exactly the budget passes, one element more doesn't
    let per = size_of::<ItemStack>();
    let fits = PACKET_MEMORY_BUDGET as usize / per;
    azalea_buf::limits::with_element_budget(|| {
        azalea_buf::limits::charge_memory::<ItemStack>(fits).unwrap();
        azalea_buf::limits::charge_memory::<ItemStack>(1).unwrap_err();
    });
    // a new packet has the whole budget again
    azalea_buf::limits::with_element_budget(|| {
        azalea_buf::limits::charge_memory::<ItemStack>(fits).unwrap();
    });
    // outside of a packet nothing is charged
    azalea_buf::limits::charge_memory::<ItemStack>(usize::MAX).unwrap();
}
