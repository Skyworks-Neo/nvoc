//! Fabric-domain attachment: which ClkDomains WRITE-record offsets ride into
//! which other domains, and how to resolve a **net** (sign-corrected) write.
//!
//! ## Where the relation comes from
//!
//! The driver declares the attachment in its private V/F table: each bank's
//! main `vf_curve` block carries optional EXTENDED-section slots
//! (`ClkVfPointPrivate::domain_freqs_mhz`, `+0x74+0x10*k`) holding the
//! *derived* operating point of the fabric domains that have no main block of
//! their own. `get-private-vftable` on an RTX 4060 (Ada/R610.74) reads
//! `bank0 xbar vf_curve … ext0=SYS ext1=HOST` — XBAR is the parent, SYS and
//! HOST its attached children: moving XBAR drags them, and their own offsets
//! stack on top of the dragged value. The slot roster is
//! `[XBAR, SYS, MSD, HOST]` minus every domain that owns a main `vf_curve`
//! block **in this table** — a layout fact, never a generation table.
//!
//! Live A/B on that 4060 confirmed the attachment for SYS and HOST and
//! **refuted** it for MSD (writing bit1 does not move MSD, and MSD is absent
//! from the xbar block's ext list — the two agree).
//!
//! ## Two sources, two jobs
//!
//! * **Structure** (which edges exist) comes from the driver table.
//! * **Availability** (whether we are willing to *write* compensations
//!   against an edge) comes from [`LEDGER`], a hand-maintained evidence log
//!   scoped to the generations the experiment ran on. The ledger never adds
//!   structure; it only gates trust, and it keeps working on generations
//!   whose ext layout is not decoded yet (Blackwell: the reader deliberately
//!   leaves the extended section alone, so the table yields no edges there
//!   while `bit1 → SYS` stays applied exactly as it did before this module
//!   existed).
//!
//! An edge that the table shows but the ledger does not cover is reported
//! (`evidence = unverified`, `applied = false`): the front-ends keep their
//! previous raw-write behavior until an A/B promotes it.
//!
//! ## Net semantics
//!
//! `net(d) = own(d) + Σ own(applied parents of d)`. A row displays and writes
//! the NET value; [`FabricTree::plan`] turns a set of net targets into the raw
//! WRITE-record values that realize them, re-parking every non-target child of
//! a moved parent so its net stays put, in topological order (parents first).

use crate::{ClkVfDomainHint, ClkVfPointsPrivate, ClkVfSegmentKind, GpuType};
use std::collections::{BTreeMap, BTreeSet};

/// Slots observed per (owner, ext slot) before the slot counts as an edge —
/// mirrors the ext-curve plausibility gate the GUI/TUI extractors use
/// (`_extract_ext_curves`), so a single stray record cannot invent a relation.
const MIN_SLOT_POINTS: usize = 4;

/// The fabric domains that can appear in the ext roster, in slot order.
///
/// **Universe warning**: these are ClkDomains **WRITE-record** bits (the
/// `clk_client_record_name` list), *not* the MEASURE/RTSS bit space and *not*
/// `GetAllClocks`'s `ClockDomainId`. The three are disjoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FabricDomain {
    Xbar,
    Sys,
    Msd,
    Host,
}

impl FabricDomain {
    /// Roster order = ext slot order.
    pub const POOL: [FabricDomain; 4] = [
        FabricDomain::Xbar,
        FabricDomain::Sys,
        FabricDomain::Msd,
        FabricDomain::Host,
    ];

    /// ClkDomains WRITE-record bit for this domain.
    pub fn bit(self) -> u8 {
        match self {
            FabricDomain::Xbar => 1,
            FabricDomain::Sys => 3,
            FabricDomain::Msd => 5,
            FabricDomain::Host => 9,
        }
    }

    pub fn from_bit(bit: u8) -> Option<Self> {
        match bit {
            1 => Some(FabricDomain::Xbar),
            3 => Some(FabricDomain::Sys),
            5 => Some(FabricDomain::Msd),
            9 => Some(FabricDomain::Host),
            _ => None,
        }
    }

    /// lowercase slug used in CLI/JSON payloads.
    pub fn slug(self) -> &'static str {
        match self {
            FabricDomain::Xbar => "xbar",
            FabricDomain::Sys => "sys",
            FabricDomain::Msd => "msd",
            FabricDomain::Host => "host",
        }
    }

    /// UPPERCASE name as it appears in the ext-slot roster legend.
    pub fn roster_name(self) -> &'static str {
        match self {
            FabricDomain::Xbar => "XBAR",
            FabricDomain::Sys => "SYS",
            FabricDomain::Msd => "MSD",
            FabricDomain::Host => "HOST",
        }
    }

    pub fn from_roster_name(name: &str) -> Option<Self> {
        FabricDomain::POOL
            .into_iter()
            .find(|d| d.roster_name().eq_ignore_ascii_case(name))
    }

    /// Map a `vf_curve` segment's empirical domain hint into this space.
    /// `Gpc`/`Mem`/`Disp`/`Unknown` have no fabric WRITE bit of their own.
    pub fn from_hint(hint: ClkVfDomainHint) -> Option<Self> {
        match hint {
            ClkVfDomainHint::Xbar => Some(FabricDomain::Xbar),
            ClkVfDomainHint::Msd => Some(FabricDomain::Msd),
            _ => None,
        }
    }
}

/// How much we trust an edge's *write-side* propagation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// Measured by a live A/B on this generation class.
    AbVerified,
    /// The driver's table shows the edge; nobody has offset the parent and
    /// watched the child. Reported, **not** compensated.
    Unverified,
    /// Measured NOT to propagate — compensate and the target lands off by the
    /// parent's offset.
    Refuted,
}

impl Evidence {
    pub fn slug(self) -> &'static str {
        match self {
            Evidence::AbVerified => "ab-verified",
            Evidence::Unverified => "unverified",
            Evidence::Refuted => "refuted",
        }
    }
}

/// Generation scope an evidence claim covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenScope {
    /// The 30系+/workstation/server class — `GpuType::is_ampere_plus`.
    AmperePlus,
    /// Ada only — `GpuType::is_ada`.
    Ada,
    /// Not a measurement: the driver table itself (derived edges).
    Structure,
}

impl GenScope {
    pub fn accepts(self, gpu: GpuType) -> bool {
        match self {
            GenScope::AmperePlus => gpu.is_ampere_plus(),
            GenScope::Ada => gpu.is_ada(),
            GenScope::Structure => true,
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            GenScope::AmperePlus => "ampere-plus",
            GenScope::Ada => "ada",
            GenScope::Structure => "structure",
        }
    }
}

/// One evidence-log entry. `note` carries the provenance so a future reader
/// can re-run the experiment or demote the edge without archaeology.
struct LedgerEntry {
    parent: FabricDomain,
    child: FabricDomain,
    evidence: Evidence,
    scope: GenScope,
    note: &'static str,
}

/// The evidence log. Deliberately tiny and hand-written: every line is a claim
/// someone measured, not a generation table. Structural edges the driver
/// declares but this list does not cover stay `Unverified` (reported only).
const LEDGER: &[LedgerEntry] = &[
    LedgerEntry {
        parent: FabricDomain::Xbar,
        child: FabricDomain::Sys,
        evidence: Evidence::AbVerified,
        scope: GenScope::AmperePlus,
        note: "slot-0 A/B (Ampere30 + Ada, 2026-08-31): bit1 moves SYS and stacks with bit3; \
               bank0 xbar vf_curve carries ext0=SYS on Ada",
    },
    LedgerEntry {
        parent: FabricDomain::Xbar,
        child: FabricDomain::Host,
        evidence: Evidence::AbVerified,
        scope: GenScope::Ada,
        note: "RTX 4060 Laptop A/B + bank0 xbar vf_curve ext1=HOST on Ada (R610.74)",
    },
    LedgerEntry {
        parent: FabricDomain::Xbar,
        child: FabricDomain::Msd,
        evidence: Evidence::Refuted,
        scope: GenScope::Ada,
        note: "RTX 4060 Laptop A/B: writing bit1 does NOT move MSD; MSD is absent from the \
               xbar block's ext roster",
    },
];

/// Resolved relation for one GPU: the driver-derived structure merged with the
/// evidence log.
#[derive(Debug, Clone, Default)]
pub struct FabricTree {
    roster: Vec<FabricDomain>,
    edges: Vec<Edge>,
    table_available: bool,
}

#[derive(Debug, Clone)]
struct Edge {
    parent: FabricDomain,
    child: FabricDomain,
    evidence: Evidence,
    scope: GenScope,
    /// Seen in *this* table's ext slots.
    derived: bool,
    note: &'static str,
}

/// Payload-ready projection of one edge (no serde dependency in core).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FabricEdgeRecord {
    pub parent: FabricDomain,
    pub child: FabricDomain,
    pub evidence: Evidence,
    pub scope: GenScope,
    /// Trusted enough to write compensations against.
    pub applied: bool,
    /// The driver's table shows this edge on this GPU (false when the table
    /// was unavailable — not the same as "the driver disagrees").
    pub derived: bool,
    pub note: &'static str,
}

impl FabricTree {
    /// Build the relation for `gpu`. `table` is the **unfiltered** private V/F
    /// table (both banks, every domain) — a `--bank/--domain` filtered view
    /// would distort the roster and must never be passed here. `None` (read
    /// unsupported / failed) leaves the evidence log standing alone.
    pub fn derive(table: Option<&ClkVfPointsPrivate>, gpu: GpuType) -> Self {
        let (roster, derived) = match table {
            Some(t) => derive_structure(t),
            None => (Vec::new(), BTreeSet::new()),
        };
        let mut edges: Vec<Edge> = Vec::new();
        // Evidence log first: its verdict outranks a structural sighting.
        for e in LEDGER {
            if !e.scope.accepts(gpu) {
                continue;
            }
            edges.push(Edge {
                parent: e.parent,
                child: e.child,
                evidence: e.evidence,
                scope: e.scope,
                derived: derived.contains(&(e.parent, e.child)),
                note: e.note,
            });
        }
        // Structural edges nobody has measured yet are reported, not applied.
        for &(parent, child) in &derived {
            if edges.iter().any(|e| e.parent == parent && e.child == child) {
                continue;
            }
            edges.push(Edge {
                parent,
                child,
                evidence: Evidence::Unverified,
                scope: GenScope::Structure,
                derived: true,
                note: "driver ext-slot attachment; no A/B yet — writes stay raw",
            });
        }
        Self {
            roster,
            edges,
            table_available: table.is_some(),
        }
    }

    pub fn table_available(&self) -> bool {
        self.table_available
    }

    /// Roster-minus-owners as read from this table (empty when unavailable).
    pub fn roster(&self) -> &[FabricDomain] {
        &self.roster
    }

    pub fn records(&self) -> Vec<FabricEdgeRecord> {
        self.edges
            .iter()
            .map(|e| FabricEdgeRecord {
                parent: e.parent,
                child: e.child,
                evidence: e.evidence,
                scope: e.scope,
                applied: e.applied(),
                derived: e.derived,
                note: e.note,
            })
            .collect()
    }

    /// Applied edges the driver's table does *not* show on a GPU where the
    /// table was readable — the signal that a driver update moved the
    /// topology out from under us. Empty when the table was unavailable
    /// (nothing to check against).
    pub fn unmatched_applied(&self) -> Vec<(FabricDomain, FabricDomain)> {
        if !self.table_available {
            return Vec::new();
        }
        self.edges
            .iter()
            .filter(|e| e.applied() && !e.derived)
            .map(|e| (e.parent, e.child))
            .collect()
    }

    /// Parents whose offset rides into `child` and that we are willing to
    /// compensate against. Empty = today's raw-write behavior.
    pub fn applied_parents(&self, child: FabricDomain) -> Vec<FabricDomain> {
        self.edges
            .iter()
            .filter(|e| e.child == child && e.applied())
            .map(|e| e.parent)
            .collect()
    }

    pub fn applied_children(&self, parent: FabricDomain) -> Vec<FabricDomain> {
        self.edges
            .iter()
            .filter(|e| e.parent == parent && e.applied())
            .map(|e| e.child)
            .collect()
    }

    /// Is any edge in this relation applied at all? Front-ends use this to
    /// skip the whole net path (and its extra reads) on generations where the
    /// relation is empty.
    pub fn has_applied_edges(&self) -> bool {
        self.edges.iter().any(|e| e.applied())
    }

    /// NET offset (kHz) of `d`: its own raw value plus every applied parent's.
    pub fn net_khz(&self, own_now: &BTreeMap<u8, i64>, d: FabricDomain) -> i64 {
        let mut total = own_now.get(&d.bit()).copied().unwrap_or(0);
        for p in self.applied_parents(d) {
            total += own_now.get(&p.bit()).copied().unwrap_or(0);
        }
        total
    }

    /// Resolve net targets into raw WRITE-record values (kHz).
    ///
    /// `own_now` is the current raw frequency-plane value per WRITE bit
    /// (missing = 0). Returns `(bit, kHz)` writes for the frequency plane in
    /// topological order (parents first), **omitting no-ops** so an unchanged
    /// target re-applies cleanly. Every non-target child of a moved parent is
    /// re-parked to hold its net constant.
    pub fn plan(
        &self,
        own_now: &BTreeMap<u8, i64>,
        targets: &BTreeMap<FabricDomain, i64>,
    ) -> Vec<(u8, i64)> {
        let mut nodes: BTreeSet<FabricDomain> = targets.keys().copied().collect();
        for d in FabricDomain::POOL {
            if own_now.contains_key(&d.bit()) {
                nodes.insert(d);
            }
        }
        for e in &self.edges {
            if e.applied() {
                nodes.insert(e.parent);
                nodes.insert(e.child);
            }
        }
        let order = self.topological(&nodes);
        let mut new_own: BTreeMap<u8, i64> = own_now.clone();
        let mut writes: Vec<(u8, i64)> = Vec::new();
        for d in order {
            let parents = self.applied_parents(d);
            let old = own_now.get(&d.bit()).copied().unwrap_or(0);
            let upstream_moved = parents.iter().any(|p| {
                new_own.get(&p.bit()).copied().unwrap_or(0)
                    != own_now.get(&p.bit()).copied().unwrap_or(0)
            });
            let target = targets.get(&d).copied();
            if target.is_none() && !upstream_moved {
                continue; // untouched and nothing above it moved
            }
            let parent_sum: i64 = parents
                .iter()
                .map(|p| new_own.get(&p.bit()).copied().unwrap_or(0))
                .sum();
            // A target's own := target − Σ parents. A dragged non-target keeps
            // the net it had: own := net_old − Σ new parents.
            let new = match target {
                Some(t) => t - parent_sum,
                None => {
                    let net_old: i64 = old
                        + parents
                            .iter()
                            .map(|p| own_now.get(&p.bit()).copied().unwrap_or(0))
                            .sum::<i64>();
                    net_old - parent_sum
                }
            };
            if new != old {
                writes.push((d.bit(), new));
            }
            new_own.insert(d.bit(), new);
        }
        writes
    }

    /// Parents before children; a cycle (never expected — the driver's fabric
    /// is a forest) is broken by dropping the back edge rather than looping.
    fn topological(&self, nodes: &BTreeSet<FabricDomain>) -> Vec<FabricDomain> {
        let mut out = Vec::new();
        let mut done: BTreeSet<FabricDomain> = BTreeSet::new();
        let mut visiting: BTreeSet<FabricDomain> = BTreeSet::new();
        for &d in nodes {
            self.visit(d, nodes, &mut out, &mut done, &mut visiting);
        }
        out
    }

    fn visit(
        &self,
        d: FabricDomain,
        nodes: &BTreeSet<FabricDomain>,
        out: &mut Vec<FabricDomain>,
        done: &mut BTreeSet<FabricDomain>,
        visiting: &mut BTreeSet<FabricDomain>,
    ) {
        if done.contains(&d) || visiting.contains(&d) {
            return; // already emitted, or a back edge in a cycle
        }
        visiting.insert(d);
        for p in self.applied_parents(d) {
            if nodes.contains(&p) {
                self.visit(p, nodes, out, done, visiting);
            }
        }
        visiting.remove(&d);
        if done.insert(d) {
            out.push(d);
        }
    }
}

impl Edge {
    fn applied(&self) -> bool {
        self.evidence == Evidence::AbVerified
    }
}

/// Read the attachment structure out of an unfiltered private V/F table:
/// `(roster, edges)`.
///
/// Roster = `[XBAR, SYS, MSD, HOST]` minus every domain owning a main
/// `vf_curve` block **in this table** (positions, not generations). A
/// `vf_curve` segment is the owner of the points inside its index range; a
/// slot `k` populated on ≥ [`MIN_SLOT_POINTS`] of them turns `owner → roster[k]`
/// into an edge.
fn derive_structure(
    table: &ClkVfPointsPrivate,
) -> (Vec<FabricDomain>, BTreeSet<(FabricDomain, FabricDomain)>) {
    let segments: Vec<_> = table
        .segments
        .iter()
        .filter(|s| s.kind == ClkVfSegmentKind::VfCurve)
        .collect();
    let owners: BTreeSet<FabricDomain> = segments
        .iter()
        .filter_map(|s| FabricDomain::from_hint(s.domain_hint))
        .collect();
    let roster: Vec<FabricDomain> = FabricDomain::POOL
        .into_iter()
        .filter(|d| !owners.contains(d))
        .collect();
    let mut counts: BTreeMap<(FabricDomain, usize), usize> = BTreeMap::new();
    for p in &table.points {
        let owner = segments
            .iter()
            .find(|s| {
                s.bank == p.bank
                    && s.start_index as usize <= p.index as usize
                    && p.index as usize <= s.end_index as usize
            })
            .and_then(|s| FabricDomain::from_hint(s.domain_hint));
        let Some(owner) = owner else { continue };
        for k in 0..roster.len().min(4).min(p.domain_freqs_mhz.len()) {
            if p.domain_freqs_mhz[k] > 0 && p.domain_volts_uV[k] > 0 {
                *counts.entry((owner, k)).or_default() += 1;
            }
        }
    }
    let edges = counts
        .into_iter()
        .filter(|&(_, n)| n >= MIN_SLOT_POINTS)
        .map(|((owner, k), _)| (owner, roster[k]))
        .collect();
    (roster, edges)
}

/// One edge as a label (`"xbar→sys"`) — shared by logs and payload fields.
pub fn edge_label(parent: FabricDomain, child: FabricDomain) -> String {
    format!("{}→{}", parent.slug(), child.slug())
}

/// Human-readable one-line summary for logs (`"xbar→sys, xbar→host"`).
pub fn describe_edges(edges: &[(FabricDomain, FabricDomain)]) -> String {
    edges
        .iter()
        .map(|&(p, c)| edge_label(p, c))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClkVfPointPrivate, ClkVfSegment, ClkVfSegmentKind};

    fn point(bank: u8, index: u16, slots: &[(usize, u32, u32)]) -> ClkVfPointPrivate {
        let mut p = ClkVfPointPrivate {
            bank,
            index,
            record_type: 8,
            ..Default::default()
        };
        for &(k, f, v) in slots {
            p.domain_freqs_mhz[k] = f;
            p.domain_volts_uV[k] = v;
        }
        p
    }

    fn seg(bank: u8, hint: ClkVfDomainHint, start: u16, end: u16) -> ClkVfSegment {
        ClkVfSegment {
            bank,
            record_type: 8,
            kind: ClkVfSegmentKind::VfCurve,
            domain_hint: hint,
            start_index: start,
            end_index: end,
            count: end - start + 1,
            ..Default::default()
        }
    }

    /// Ada/R610.74 shape: gpc + xbar + msd all have main blocks in bank 0, the
    /// xbar block carries ext0=SYS, ext1=HOST (live 4060 dump).
    fn ada_table() -> ClkVfPointsPrivate {
        let mut t = ClkVfPointsPrivate {
            segments: vec![
                seg(0, ClkVfDomainHint::Gpc, 0, 126),
                seg(0, ClkVfDomainHint::Xbar, 127, 253),
                seg(0, ClkVfDomainHint::Msd, 254, 380),
            ],
            ..Default::default()
        };
        for i in 0..8u16 {
            t.points.push(point(
                0,
                127 + i,
                &[(0, 2400 + i as u32, 1_000_000), (1, 900, 800_000)],
            ));
        }
        t
    }

    /// Ampere shape: only gpc + xbar own main blocks, xbar fills 3 slots.
    fn ampere_table() -> ClkVfPointsPrivate {
        let mut t = ClkVfPointsPrivate {
            segments: vec![
                seg(0, ClkVfDomainHint::Gpc, 0, 126),
                seg(0, ClkVfDomainHint::Xbar, 127, 253),
            ],
            ..Default::default()
        };
        for i in 0..8u16 {
            t.points.push(point(
                0,
                127 + i,
                &[(0, 2400, 1_000_000), (1, 1900, 900_000), (2, 1600, 850_000)],
            ));
        }
        t
    }

    /// Turing shape: the gpc block is the only main block, 4 slots = the
    /// whole pool.
    fn turing_table() -> ClkVfPointsPrivate {
        let mut t = ClkVfPointsPrivate {
            segments: vec![seg(0, ClkVfDomainHint::Gpc, 0, 126)],
            ..Default::default()
        };
        for i in 0..8u16 {
            t.points.push(point(
                0,
                i,
                &[
                    (0, 2400, 1_000_000),
                    (1, 1900, 900_000),
                    (2, 1600, 850_000),
                    (3, 1300, 800_000),
                ],
            ));
        }
        t
    }

    fn own(pairs: &[(u8, i64)]) -> BTreeMap<u8, i64> {
        pairs.iter().copied().collect()
    }

    fn targets(pairs: &[(FabricDomain, i64)]) -> BTreeMap<FabricDomain, i64> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn ada_roster_and_edges_match_the_live_dump() {
        let tree = FabricTree::derive(Some(&ada_table()), GpuType::Mobile40Series);
        assert!(tree.table_available());
        assert_eq!(tree.roster(), [FabricDomain::Sys, FabricDomain::Host]);
        let applied: Vec<_> = tree
            .records()
            .into_iter()
            .filter(|r| r.applied)
            .map(|r| (r.parent, r.child, r.evidence, r.derived))
            .collect();
        assert_eq!(
            applied,
            vec![
                (
                    FabricDomain::Xbar,
                    FabricDomain::Sys,
                    Evidence::AbVerified,
                    true
                ),
                (
                    FabricDomain::Xbar,
                    FabricDomain::Host,
                    Evidence::AbVerified,
                    true
                ),
            ]
        );
        assert_eq!(tree.applied_parents(FabricDomain::Msd), vec![]);
        assert!(tree.unmatched_applied().is_empty());
    }

    #[test]
    fn ampere_derives_three_children_but_only_sys_is_trusted() {
        let tree = FabricTree::derive(Some(&ampere_table()), GpuType::Mobile30Series);
        assert_eq!(
            tree.roster(),
            [FabricDomain::Sys, FabricDomain::Msd, FabricDomain::Host]
        );
        assert_eq!(
            tree.applied_parents(FabricDomain::Sys),
            vec![FabricDomain::Xbar]
        );
        // derived but unmeasured → reported, not compensated
        assert_eq!(tree.applied_parents(FabricDomain::Msd), vec![]);
        assert_eq!(tree.applied_parents(FabricDomain::Host), vec![]);
        let unverified: Vec<_> = tree
            .records()
            .into_iter()
            .filter(|r| r.evidence == Evidence::Unverified && r.derived)
            .map(|r| (r.parent, r.child))
            .collect();
        assert_eq!(
            unverified,
            vec![
                (FabricDomain::Xbar, FabricDomain::Msd),
                (FabricDomain::Xbar, FabricDomain::Host),
            ]
        );
    }

    #[test]
    fn turing_structure_is_reported_but_never_applied() {
        let tree = FabricTree::derive(Some(&turing_table()), GpuType::Mobile20Series);
        assert_eq!(
            tree.roster(),
            [
                FabricDomain::Xbar,
                FabricDomain::Sys,
                FabricDomain::Msd,
                FabricDomain::Host
            ]
        );
        assert!(tree.records().iter().all(|r| !r.applied));
        assert!(!tree.has_applied_edges());
        // GPC is not a fabric WRITE bit — the gpc-owned ext slots stay
        // reported-only, so a Core offset never re-parks fabric records.
        let plan = tree.plan(
            &own(&[(1, 50_000)]),
            &targets(&[(FabricDomain::Sys, 30_000)]),
        );
        assert_eq!(plan, vec![(3, 30_000)]);
    }

    #[test]
    fn blackwell_without_ext_keeps_the_30series_edge() {
        // The reader leaves the extended section alone on Blackwell: no table,
        // no derived edges — the evidence log alone must keep bit1→SYS alive.
        let tree = FabricTree::derive(None, GpuType::Mobile50Series);
        assert!(!tree.table_available());
        assert!(tree.roster().is_empty());
        assert_eq!(
            tree.applied_parents(FabricDomain::Sys),
            vec![FabricDomain::Xbar]
        );
        assert_eq!(tree.applied_parents(FabricDomain::Host), vec![]);
        assert!(tree.unmatched_applied().is_empty()); // nothing to check against
        let plan = tree.plan(
            &own(&[(1, 100_000), (3, 20_000)]),
            &targets(&[(FabricDomain::Sys, 30_000)]),
        );
        assert_eq!(plan, vec![(3, -70_000)]);
    }

    #[test]
    fn applied_parent_reports_a_stale_driver_table() {
        // An Ada table whose xbar block no longer carries the SYS slot: the
        // ledger edge still applies (driver-version drift is worth surfacing,
        // not silently obeying).
        let mut t = ada_table();
        for p in t.points.iter_mut() {
            p.domain_freqs_mhz[0] = 0;
            p.domain_volts_uV[0] = 0;
        }
        let tree = FabricTree::derive(Some(&t), GpuType::Mobile40Series);
        assert_eq!(
            tree.unmatched_applied(),
            vec![(FabricDomain::Xbar, FabricDomain::Sys)]
        );
        assert_eq!(
            tree.applied_parents(FabricDomain::Sys),
            vec![FabricDomain::Xbar]
        );
    }

    // ── plan() must reproduce the shipped single-edge formulas exactly ──

    fn ada_tree() -> FabricTree {
        FabricTree::derive(Some(&ada_table()), GpuType::Mobile40Series)
    }

    #[test]
    fn xbar_write_parks_the_child_cancels_and_holds_both_nets() {
        let tree = ada_tree();
        // Pre-write: no Xbar offset yet, SYS net −40 MHz, HOST net −10 MHz.
        // Writing Xbar = +200 MHz must leave both nets exactly where they were.
        let now = own(&[(1, 0), (3, -40_000), (9, -10_000)]);
        let plan = tree.plan(&now, &targets(&[(FabricDomain::Xbar, 200_000)]));
        assert_eq!(plan, vec![(1, 200_000), (3, -240_000), (9, -210_000)]);
        let after: BTreeMap<u8, i64> = now
            .iter()
            .map(|(k, v)| (*k, *v))
            .chain(plan.iter().copied())
            .collect();
        assert_eq!(tree.net_khz(&after, FabricDomain::Sys), -40_000);
        assert_eq!(tree.net_khz(&after, FabricDomain::Host), -10_000);
    }

    #[test]
    fn xbar_write_is_idempotent() {
        let tree = ada_tree();
        let now = own(&[(1, 200_000), (3, -40_000), (9, -150_000)]);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Xbar, 200_000)])),
            vec![]
        );
    }

    #[test]
    fn sys_write_resolves_the_net_against_the_parent() {
        let tree = ada_tree();
        let now = own(&[(1, 120_000), (3, 30_000), (9, 0)]);
        // net SYS target +80 → bit3 := 80_000 − 120_000 = −40_000
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Sys, 80_000)])),
            vec![(3, -40_000)]
        );
    }

    #[test]
    fn host_write_is_now_net_aware_on_ada() {
        let tree = ada_tree();
        let now = own(&[(1, 100_000), (3, 0), (9, 0)]);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Host, 50_000)])),
            vec![(9, -50_000)]
        );
    }

    #[test]
    fn resetting_a_row_zeroes_its_net_and_keeps_the_siblings() {
        let tree = ada_tree();
        // Xbar ↺: bit1 := 0; SYS net (−100_000+? no: 100_000−100_000=0) and HOST
        // net 140_000 both survive — bit3/bit9 absorb the parent's departure.
        let now = own(&[(1, 100_000), (3, -100_000), (9, 40_000)]);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Xbar, 0)])),
            vec![(1, 0), (3, 0), (9, 140_000)]
        );
        let after = own(&[(1, 0), (3, 0), (9, 140_000)]);
        assert_eq!(tree.net_khz(&after, FabricDomain::Sys), 0);
        assert_eq!(tree.net_khz(&after, FabricDomain::Host), 140_000);
        // Sys ↺ with a live SYS net: bit3 := −bit1; XBAR/HOST untouched. This
        // is the write today's `_reset_sys_domain_action` performs, and it is
        // now the *net* zero (old code also wrote bit3 := 0 → net became bit1).
        let now = own(&[(1, 100_000), (3, 30_000), (9, 40_000)]);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Sys, 0)])),
            vec![(3, -100_000)]
        );
        // …and an already-zero SYS net needs no write at all (idempotent ↺).
        let now = own(&[(1, 100_000), (3, -100_000), (9, 40_000)]);
        assert_eq!(tree.plan(&now, &targets(&[(FabricDomain::Sys, 0)])), vec![]);
    }

    #[test]
    fn joint_targets_solve_in_parent_first_order() {
        let tree = ada_tree();
        let now = own(&[(1, 0), (3, 0), (9, 0)]);
        let plan = tree.plan(
            &now,
            &targets(&[(FabricDomain::Xbar, 100_000), (FabricDomain::Sys, 30_000)]),
        );
        // bit1 := 100; bit3 := 30 − 100 = −70; HOST keeps net 0 → bit9 := −100
        assert_eq!(plan, vec![(1, 100_000), (3, -70_000), (9, -100_000)]);
    }

    #[test]
    fn zeroing_every_own_zeroes_every_net() {
        // "reset all domains" with a complete read of the four records.
        let tree = ada_tree();
        let now = own(&[(1, 100_000), (3, -100_000), (5, 70_000), (9, 40_000)]);
        let all: BTreeMap<FabricDomain, i64> =
            FabricDomain::POOL.into_iter().map(|d| (d, 0)).collect();
        let plan = tree.plan(&now, &all);
        assert_eq!(plan, vec![(1, 0), (3, 0), (5, 0), (9, 0)]);
        let after: BTreeMap<u8, i64> = plan.iter().copied().collect();
        assert_eq!(tree.net_khz(&after, FabricDomain::Sys), 0);
        assert_eq!(tree.net_khz(&after, FabricDomain::Host), 0);
    }

    #[test]
    fn missing_records_read_as_zero_and_still_hold_their_net() {
        // `own_now` is the caller's read of each record; an absent bit is 0,
        // not "skip me" — so a parent move still parks that record's believed
        // net instead of silently dropping it. (Front-ends must therefore
        // hand in every record they claim to manage: the four fabric rows all
        // come from one `get-private-freq-domain-info` read.)
        let tree = ada_tree();
        let now = own(&[(1, 100_000), (9, 40_000)]); // bit3 never read → 0
        assert_eq!(tree.net_khz(&now, FabricDomain::Sys), 100_000);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Xbar, 0)])),
            vec![(1, 0), (3, 100_000), (9, 140_000)]
        );
    }

    #[test]
    fn net_khz_sums_applied_parents_only() {
        let tree = ada_tree();
        let now = own(&[(1, 100_000), (3, 20_000), (5, 70_000), (9, -30_000)]);
        assert_eq!(tree.net_khz(&now, FabricDomain::Sys), 120_000);
        assert_eq!(tree.net_khz(&now, FabricDomain::Host), 70_000);
        assert_eq!(tree.net_khz(&now, FabricDomain::Msd), 70_000); // refuted → raw
        assert_eq!(tree.net_khz(&now, FabricDomain::Xbar), 100_000);
    }

    #[test]
    fn stray_single_record_does_not_invent_an_edge() {
        let mut t = ada_table();
        t.points.push(point(0, 250, &[(2, 2600, 1_100_000)])); // one HOST-ish slot
        let tree = FabricTree::derive(Some(&t), GpuType::Mobile40Series);
        assert_eq!(tree.roster(), [FabricDomain::Sys, FabricDomain::Host]);
        assert!(
            tree.records().iter().all(|r| (r.parent, r.child)
                != (FabricDomain::Xbar, FabricDomain::Host)
                || r.derived)
        );
    }

    #[test]
    fn domains_map_to_the_write_record_bits() {
        assert_eq!(FabricDomain::from_bit(9), Some(FabricDomain::Host));
        assert_eq!(
            FabricDomain::from_roster_name("host"),
            Some(FabricDomain::Host)
        );
        assert_eq!(FabricDomain::from_hint(ClkVfDomainHint::Gpc), None);
        assert_eq!(
            FabricDomain::from_hint(ClkVfDomainHint::Xbar),
            Some(FabricDomain::Xbar)
        );
        assert_eq!(FabricDomain::Xbar.bit(), 1);
        assert_eq!(FabricDomain::Sys.bit(), 3);
        assert_eq!(FabricDomain::Msd.bit(), 5);
        assert_eq!(FabricDomain::Host.bit(), 9);
    }
}
