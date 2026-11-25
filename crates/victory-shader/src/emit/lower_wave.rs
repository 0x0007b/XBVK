//! Wave64-on-Wave32 helpers for [`Lower`]: LDS flag regions (`ztrx#36`)
//! and two-host-wave `ds_(b)permute_b32` table lowering (`ztrx#43`).

use std::collections::{HashMap, HashSet};

use super::{Bin, Fields, Inst, Lower, Operand, Program, Reg, Structured, V, Value, Why, need};
use crate::ir::EXEC;
use crate::structure::LaneExit;

/// LDS accesses of a console wave run as two host waves, where some sit in blocks that only
/// some lanes run (`all` is [`super::all_lane_blocks`]): per IR block, whether the host waves
/// meet (a group barrier) at its start and before its exit. `None` if barriers cannot keep the
/// console's order.
///
/// The console runs the wave's lanes in step, so an LDS write is seen by every later read. Two
/// host waves keep that order if they meet between any two accesses of which one writes. A
/// barrier must be run by every thread, so it cannot stand in a lane branch's region; but
/// every way into and out of such a region passes a block all lanes run, and the waves meet
/// there: before the exit of an all-lane block into a region with LDS accesses, and at the
/// start of an all-lane block entered from one. Accesses in all-lane blocks get their own
/// barrier as before. What that cannot order is a write and a read inside one region (blocks
/// joined without an all-lane block between them): refused.
/// shortcut: two writes in one region are taken not to hit the same dword from both halves.
pub(super) fn lds_meetings(p: &Program, st: &Structured, all: &[bool]) -> Option<Vec<(bool, bool)>> {
    let n = st.exits.len();
    let succ = |b: usize| -> Vec<usize> {
        match st.exits[b] {
            LaneExit::Jump(t) => vec![t],
            LaneExit::Branch { taken, not_taken, .. } => vec![taken, not_taken],
            LaneExit::End | LaneExit::Indirect => Vec::new(),
        }
    };
    // regions: blocks not every lane runs, joined by the edges between them
    let mut region: Vec<usize> = (0..n).collect();
    fn find(region: &mut [usize], mut b: usize) -> usize {
        while region[b] != b {
            region[b] = region[region[b]];
            b = region[b];
        }
        b
    }
    for b in 0..n {
        for t in succ(b) {
            if !all[b] && !all[t] {
                let (x, y) = (find(&mut region, b), find(&mut region, t));
                region[x] = y;
            }
        }
    }
    // per region: LDS reads, LDS writes
    let mut reads = vec![false; n];
    let mut writes = vec![false; n];
    for (b, block) in p.blocks.iter().enumerate().take(n) {
        if all[b] {
            continue;
        }
        let r = find(&mut region, b);
        for i in &block.insts {
            let name = i.name();
            if !matches!(i.fields, Fields::Ds { .. }) || name.starts_with("ds_swizzle") || name.contains("permute") {
                continue;
            }
            if name.starts_with("ds_read") {
                reads[r] = true;
            } else {
                writes[r] = true;
            }
        }
    }
    // A region that reads and writes LDS is still taken when it only raises flags
    // (`lds_flag_region`): a read that misses the other host wave's write changes nothing
    // the region leaves behind.
    let flags_on = std::env::var_os("VICTORY_LDS_FLAGS").is_none_or(|v| v != "0");
    for r in (0..n).filter(|&r| reads[r] && writes[r]).collect::<Vec<_>>() {
        let blocks: Vec<usize> = (0..n).filter(|&b| !all[b] && find(&mut region, b) == r).collect();
        if !(flags_on && lds_flag_region(p, st, &blocks)) {
            return None;
        }
    }
    let mut meet = vec![(false, false); n];
    for b in 0..n {
        for t in succ(b) {
            if all[b] && !all[t] {
                let r = find(&mut region, t);
                meet[b].1 |= reads[r] || writes[r];
            } else if !all[b] && all[t] {
                let r = find(&mut region, b);
                meet[t].0 |= reads[r] || writes[r];
            }
        }
    }
    Some(meet)
}

/// Whether a lane-branch region (`blocks`, see [`lds_meetings`]) that both reads and
/// writes LDS only raises flags, so that it may run as two host waves with group barriers at its
/// edges alone. The console runs such a region as one wave, where a lane's read sees every
/// write before it; split in two, a read may miss the other half's write. That changes nothing
/// the region leaves behind when:
/// - every LDS access in it is a `ds_read_b32` or `ds_write_b32` at a constant address, and
///   every write stores the same constant;
/// - one address at most is both read and written in it (the flag), and every write a lane
///   can reach after one of its reads goes to that flag;
/// - it has no other effect (no store, atomic, export, message or barrier);
/// - no value it computes is used after it.
///
/// A missed write only makes a lane go on as if the flag were still down; whatever that lane
/// writes is the flag's own constant, which the write it missed has already put there. So the
/// flag ends up raised exactly when some lane raises it, as on the console, and no other word
/// or register depends on when it was seen.
pub(super) fn lds_flag_region(p: &Program, st: &Structured, blocks: &[usize]) -> bool {
    let inside: HashSet<usize> = blocks.iter().copied().collect();
    // where each value is written: (block, instruction)
    let mut def: HashMap<Value, (usize, usize)> = HashMap::new();
    for (b, block) in p.blocks.iter().enumerate() {
        for (i, inst) in block.insts.iter().enumerate() {
            for &(_, v) in &inst.writes {
                def.insert(v, (b, i));
            }
        }
    }
    let inline = |op: &Operand| -> Option<u32> {
        match *op {
            Operand::Const(c @ 128..=192) => Some(u32::from(c) - 128),
            Operand::Const(c @ 193..=208) => Some((-(i32::from(c) - 192)) as u32),
            Operand::Literal(x) => Some(x),
            _ => None,
        }
    };
    // a VGPR value a `v_mov_b32` of a constant wrote, unguarded or under the same guard as `at`
    let constant = |v: Option<Value>, at: (usize, usize)| -> Option<u32> {
        let (b, i) = *def.get(&v?)?;
        let inst = &p.blocks[b].insts[i];
        if inst.name() != "v_mov_b32" {
            return None;
        }
        let guard = st.guards.get(b).and_then(|g| g.get(i)).copied().flatten();
        let same = guard.is_none() || (b == at.0 && guard == st.guards[at.0][at.1]);
        let Fields::Valu(valu) = &inst.fields else {
            return None;
        };
        if !same || valu.dpp.is_some() || valu.sdwa.is_some() {
            return None;
        }
        inline(valu.src.first()?)
    };
    // every LDS access: (block, index, is_read, address)
    let mut access: Vec<(usize, usize, bool, u32)> = Vec::new();
    let mut stored: Option<u32> = None;
    for &b in blocks {
        for (i, inst) in p.blocks[b].insts.iter().enumerate() {
            let name = inst.name();
            if name.contains("store")
                || name.contains("atomic")
                || name.starts_with("exp")
                || name.starts_with("s_sendmsg")
                || name == "s_barrier"
            {
                return false;
            }
            let Fields::Ds {
                addr,
                data,
                offset0,
                offset1,
                gds,
                ..
            } = &inst.fields
            else {
                if name.starts_with("ds_") {
                    return false;
                }
                continue;
            };
            if name.starts_with("ds_swizzle") || name.contains("permute") {
                continue;
            }
            let is_read = match name {
                "ds_read_b32" => true,
                "ds_write_b32" => false,
                _ => return false,
            };
            if *gds {
                return false;
            }
            let Some(Operand::Vector { reg, .. }) = *addr else {
                return false;
            };
            let Some(base) = constant(inst.read(Reg::V(reg)), (b, i)) else {
                return false;
            };
            let at = base.wrapping_add(u32::from(*offset0) | u32::from(*offset1) << 8);
            if !is_read {
                let Some(&Operand::Vector { reg, .. }) = data.first() else {
                    return false;
                };
                let Some(k) = constant(inst.read(Reg::V(reg)), (b, i)) else {
                    return false;
                };
                if stored.is_some_and(|s| s != k) {
                    return false;
                }
                stored = Some(k);
            }
            access.push((b, i, is_read, at));
        }
    }
    let read_at: HashSet<u32> = access.iter().filter(|a| a.2).map(|a| a.3).collect();
    let written_at: HashSet<u32> = access.iter().filter(|a| !a.2).map(|a| a.3).collect();
    let flags: Vec<u32> = read_at.intersection(&written_at).copied().collect();
    if flags.len() > 1 {
        return false;
    }
    if let Some(&flag) = flags.first() {
        // the blocks of the region a lane can reach from `b` along one edge or more
        let succ = |b: usize| -> Vec<usize> {
            match st.exits[b] {
                LaneExit::Jump(t) => vec![t],
                LaneExit::Branch {
                    taken, not_taken, ..
                } => vec![taken, not_taken],
                LaneExit::End | LaneExit::Indirect => Vec::new(),
            }
        };
        for &(rb, ri, is_read, _) in &access {
            if !is_read {
                continue;
            }
            let mut seen: HashSet<usize> = HashSet::new();
            let mut work: Vec<usize> = succ(rb)
                .into_iter()
                .filter(|t| inside.contains(t))
                .collect();
            while let Some(b) = work.pop() {
                if seen.insert(b) {
                    work.extend(succ(b).into_iter().filter(|t| inside.contains(t)));
                }
            }
            let after = access.iter().any(|&(wb, wi, w_read, at)| {
                !w_read && at != flag && ((wb == rb && wi > ri) || seen.contains(&wb))
            });
            if after {
                return false;
            }
        }
    }
    // Nothing the region computes is used after it: no register it writes is live, lane by
    // lane, where a lane leaves it. A lane's own write kills a register for that lane (an
    // unguarded instruction that writes the whole dword); what an instruction reads of a VGPR
    // only for the lanes outside EXEC (the old value of the one it writes) is no use.
    let succ = |b: usize| -> Option<Vec<usize>> {
        match st.exits[b] {
            LaneExit::Jump(t) => Some(vec![t]),
            LaneExit::Branch {
                taken, not_taken, ..
            } => Some(vec![taken, not_taken]),
            LaneExit::End => Some(Vec::new()),
            LaneExit::Indirect => None,
        }
    };
    let n = st.exits.len().min(p.blocks.len());
    if (0..n).any(|b| succ(b).is_none()) {
        return false;
    }
    let exec = |r: Reg| matches!(r, Reg::S(c) if c == EXEC || c == EXEC + 1);
    let covers = |ops: &[Operand], r: Reg| {
        let Reg::V(x) = r else { return false };
        ops.iter().any(|op| {
            matches!(*op, Operand::Vector { reg, dwords }
                if (u16::from(reg)..u16::from(reg) + u16::from(dwords.max(1))).contains(&u16::from(x)))
        })
    };
    // per instruction: the registers it reads for its lanes, and those it writes in full
    let effect = |b: usize, i: usize| -> (Vec<Reg>, Vec<Reg>) {
        let inst = &p.blocks[b].insts[i];
        let name = inst.name();
        let partial = name.contains("16")
            || name.contains("mac")
            || name.contains("lane")
            || matches!(&inst.fields, Fields::Valu(v) if v.dpp.is_some() || v.sdwa.is_some());
        // the operands it reads explicitly, where its old destination is not one of them
        let sources: Option<Vec<Operand>> = match &inst.fields {
            Fields::Valu(v) if !partial => Some(v.src.clone()),
            Fields::Ds { addr, data, .. } if !partial => {
                Some(addr.iter().copied().chain(data.iter().copied()).collect())
            }
            Fields::Buffer { addr, data, .. }
                if !partial && !name.contains("store") && !name.contains("atomic") =>
            {
                let _ = data;
                Some(vec![*addr])
            }
            Fields::Image { addr, .. }
                if !partial && !name.contains("store") && !name.contains("atomic") =>
            {
                Some(addr.clone())
            }
            _ => None,
        };
        let mut reads = Vec::new();
        for &(r, _) in &inst.reads {
            if exec(r) {
                continue;
            }
            let old_only = matches!(r, Reg::V(_))
                && inst.write(r).is_some()
                && sources.as_ref().is_some_and(|src| !covers(src, r));
            if !old_only {
                reads.push(r);
            }
        }
        let whole = st
            .guards
            .get(b)
            .and_then(|g| g.get(i))
            .copied()
            .flatten()
            .is_none();
        let kills: Vec<Reg> = if whole
            && (sources.is_some()
                || !matches!(
                    inst.fields,
                    Fields::Valu(_) | Fields::Ds { .. } | Fields::Buffer { .. } | Fields::Image { .. }
                )) {
            inst.writes
                .iter()
                .map(|w| w.0)
                .filter(|&r| !exec(r) && !(partial && matches!(r, Reg::V(_))))
                .collect()
        } else {
            Vec::new()
        };
        (reads, kills)
    };
    let mut live_in: Vec<HashSet<Reg>> = vec![HashSet::new(); n];
    loop {
        let mut changed = false;
        for b in (0..n).rev() {
            let mut live: HashSet<Reg> = HashSet::new();
            for t in succ(b).unwrap_or_default() {
                if let Some(l) = live_in.get(t) {
                    live.extend(l.iter().copied());
                }
            }
            for i in (0..p.blocks[b].insts.len()).rev() {
                let (reads, kills) = effect(b, i);
                for r in kills {
                    live.remove(&r);
                }
                live.extend(reads);
            }
            if live != live_in[b] {
                live_in[b] = live;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let written: HashSet<Reg> = blocks
        .iter()
        .flat_map(|&b| {
            p.blocks[b]
                .insts
                .iter()
                .flat_map(|i| i.writes.iter().map(|w| w.0))
        })
        .filter(|&r| !exec(r))
        .collect();
    let mut leaks: Vec<(usize, Reg)> = Vec::new();
    for &b in blocks {
        for t in succ(b).unwrap_or_default() {
            if inside.contains(&t) {
                continue;
            }
            for r in &live_in[t] {
                if written.contains(r) {
                    leaks.push((t, *r));
                }
            }
        }
    }
    if std::env::var_os("VICTORY_WAVE_DUMP").is_some() {
        eprintln!(
            "WAVE64-LDS-FLAGS blocks={blocks:?} flag={flags:?} stored={stored:?} leaks={leaks:?}"
        );
    }
    leaks.is_empty()
}

impl<'a> Lower<'a> {
    /// Checks two-host-wave permute preconditions and returns the console wave's
    /// 64-dword permute table base along with `(at, x, cl, lds)`.
    fn permute_table_setup(
        &mut self,
        inst: &Inst,
        addr: &Option<Operand>,
        data: &[Operand],
        at_block: usize,
    ) -> Result<(V, V, V, V, V), Why> {
        if self.local {
            // a console wave of 64 run as two host waves that do not
            // exchange (each as a wave of its own): the lane named may
            // be in the other one
            return Err("a lane permute across two host waves");
        }
        if !self.all_lanes(self.cur_ir) {
            // the barriers / table need every lane of the group
            need(self.cur_ir);
            return Err("a lane permute across two host waves where not every lane runs");
        }
        let table = self
            .permute_table
            .ok_or("a lane permute across two host waves")?;
        let at = self.vgprs(inst, addr.as_ref().ok_or("a permute address")?, 1)?[0];
        let x = self.vgprs(inst, data.first().ok_or("a permute source")?, 1)?[0];
        let cl = self.console_lane();
        let six = self.s.m.i32(6);
        let thread = self.console_thread();
        let slot = self.s.m.i32(table);
        // this console wave's 64 table dwords, as `exchange_pair` picks
        // its pair of exchange dwords: the wave's threads are 64 apart
        let wave = self.s.m.bin(at_block, Bin::LShr, thread, six);
        let base = self.s.m.bin(at_block, Bin::Shl, wave, six);
        let base = self.s.m.bin(at_block, Bin::Add, base, slot);
        let lds = self.s.lds();
        Ok((base, at, x, cl, lds))
    }

    /// Computes `((at + offset) >> 2) & 63` in DXIL block `block`.
    fn permute_slot(&mut self, block: usize, at: V, offset: u32) -> V {
        let o = self.s.m.i32(offset);
        let at = self.s.m.bin(block, Bin::Add, at, o);
        let two = self.s.m.i32(2);
        let lane = self.s.m.bin(block, Bin::LShr, at, two);
        let last = self.s.m.i32(63);
        self.s.m.bin(block, Bin::And, lane, last)
    }

    /// Backward permute (`ds_bpermute_b32`) across two 32-lane host waves.
    pub(super) fn bpermute_halves(
        &mut self,
        inst: &Inst,
        addr: &Option<Operand>,
        data: &[Operand],
        offset: u32,
        guard: Option<V>,
        dst_reg: Option<u8>,
    ) -> Result<(), Why> {
        let c = self.cur;
        let (base, at, x, cl, lds) = self.permute_table_setup(inst, addr, data, c)?;
        let x = match guard {
            Some(g) => {
                let z = self.s.m.i32(0);
                self.s.m.select(c, g, x, z)
            }
            None => x,
        };
        let index = self.s.m.bin(c, Bin::Add, base, cl);
        let ptr = self.s.m.element(c, lds, index);
        self.s.m.store(c, ptr, x);
        let c2 = self.cur;
        self.group_sync(c2);
        let src = self.permute_slot(c2, at, offset);
        let index = self.s.m.bin(c2, Bin::Add, base, src);
        let ptr = self.s.m.element(c2, lds, index);
        let r = self.s.m.load(c2, ptr);
        // the slots are written again only after every thread read them
        let c3 = self.cur;
        self.group_sync(c3);
        let reg = dst_reg.ok_or("a permute result")?;
        self.write(inst, Reg::V(reg), r, guard);
        Ok(())
    }

    /// Forward permute (`ds_permute_b32`) across two 32-lane host waves.
    pub(super) fn permute_halves(
        &mut self,
        inst: &Inst,
        addr: &Option<Operand>,
        data: &[Operand],
        offset: u32,
        guard: Option<V>,
        dst_reg: Option<u8>,
    ) -> Result<(), Why> {
        let c = self.cur;
        let (base, at, x, cl, lds) = self.permute_table_setup(inst, addr, data, c)?;
        let to = self.permute_slot(c, at, offset);
        let index = self.s.m.bin(c, Bin::Add, base, to);
        let ptr = self.s.m.element(c, lds, index);
        let g = self.begin_guarded(guard);
        self.s.m.store(self.cur, ptr, x);
        self.end_guarded(g, &[]);
        let c2 = self.cur;
        self.group_sync(c2);
        // every lane takes what was written to its own slot
        let index = self.s.m.bin(c2, Bin::Add, base, cl);
        let ptr = self.s.m.element(c2, lds, index);
        let r = self.s.m.load(c2, ptr);
        // the slots are written again only after every thread read them
        let c3 = self.cur;
        self.group_sync(c3);
        let reg = dst_reg.ok_or("a permute result")?;
        self.write(inst, Reg::V(reg), r, guard);
        Ok(())
    }
}
