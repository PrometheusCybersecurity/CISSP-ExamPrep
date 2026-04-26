//! Adaptive distribution + difficulty progression.
//!
//! Pure-function port of the JS engine (`static/index.html` `computeDistribution`,
//! `buildBatchPlan`, `applyDifficultyProgression`) so the same numbers come out
//! regardless of where the calculation runs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ROLLING_WINDOW: usize = 10;
pub const PROMOTE_THRESHOLD: f64 = 0.80;
pub const DEMOTE_THRESHOLD: f64 = 0.40;
pub const BATCH_SIZE: u32 = 50;
pub const COUNT_FLOOR: u32 = 2;
pub const COUNT_CEIL: u32 = 15;

pub const DOMAINS: [(u8, &str, &str); 8] = [
    (1, "Security & Risk Management",          "Risk Mgmt"),
    (2, "Asset Security",                      "Asset Sec"),
    (3, "Security Architecture & Engineering", "Arch / Eng"),
    (4, "Communication & Network Security",    "Network"),
    (5, "Identity & Access Management",        "IAM"),
    (6, "Security Assessment & Testing",       "Assess"),
    (7, "Security Operations",                 "SecOps"),
    (8, "Software Development Security",       "SDLC"),
];

pub fn domain_name(d: u8) -> &'static str {
    DOMAINS.iter().find(|x| x.0 == d).map(|x| x.1).unwrap_or("Unknown")
}

pub fn domain_short(d: u8) -> &'static str {
    DOMAINS.iter().find(|x| x.0 == d).map(|x| x.2).unwrap_or("?")
}

pub fn tier_name(t: u8) -> &'static str {
    match t {
        1 => "Easy",
        2 => "Moderate",
        3 => "Hard",
        4 => "Expert",
        _ => "?",
    }
}

pub fn tier_desc(t: u8) -> &'static str {
    match t {
        1 => "Recall / definitions phrased in a one-sentence business context (e.g. \"Which control type is a security awareness program?\"). One clearly correct answer; distractors share the same category but are easy to rule out. Still uses CISSP vocabulary, not technician trivia.",
        2 => "Two-to-four sentence scenario with a single decision point. Two of four distractors are close in topic but wrong in scope; uses softer qualifiers (\"BEST\", \"PRIMARY\"). Requires picking the manager-level concern rather than the technical detail.",
        3 => "Multi-sentence scenario with competing pressures (cost, time, regulation, executive demand). At least two distractors are technically correct but solve a different problem; the right answer addresses ROOT CAUSE or the FIRST step in a lifecycle. Uses \"MOST\", \"FIRST\", \"NEXT\" qualifiers; references real frameworks (NIST RMF, ISO 27001, BCP/DR phases, OWASP, Saltzer & Schroeder, STRIDE) where relevant.",
        4 => "Real-exam-style scenario, typically 4-7 sentences with at least one piece of misdirecting context (urgent breach, angry executive, sensitive data type that ISN'T the pillar under attack). Two distractors are seductive technician answers; one is the obvious-but-wrong manager answer; the correct option reflects CISO judgement, balances CIA against business impact, and is the FIRST/MOST/BEST action in the lifecycle phase implied by the stem.",
        _ => "",
    }
}

/// Curated topic anchors per CISSP domain. The prompt cycles through these so
/// each batch gets broad coverage instead of (e.g.) ten flavors of the same
/// risk-register question.
pub fn domain_anchors(d: u8) -> &'static [&'static str] {
    match d {
        1 => &[
            "risk management lifecycle (identify, assess, respond, monitor)",
            "qualitative vs quantitative risk (ALE, SLE, ARO)",
            "security governance, policies vs standards vs procedures",
            "due care vs due diligence",
            "laws & regs (GDPR, HIPAA, SOX, PCI-DSS, GLBA)",
            "intellectual property (trade secret, patent, copyright, trademark)",
            "professional ethics (ISC2 code, organizational ethics)",
            "BCP scope, BIA (RTO, RPO, MTD, WRT)",
            "third-party / supply-chain risk",
            "security awareness, training, education",
        ],
        2 => &[
            "data classification & labeling",
            "data owner vs data custodian vs data steward roles",
            "data states (at rest, in transit, in use) and protection mappings",
            "data retention, archival, and secure disposal",
            "media sanitization (clear, purge, destroy; NIST SP 800-88)",
            "asset inventory & lifecycle",
            "DRM and IRM controls",
            "PII / PHI / PCI handling",
            "data minimization & masking / tokenization",
            "cloud shared-responsibility for data",
        ],
        3 => &[
            "security models (Bell-LaPadula, Biba, Clark-Wilson, Brewer-Nash)",
            "Saltzer & Schroeder secure-design principles",
            "trusted computing base, reference monitor, security kernel",
            "cryptography fundamentals (symmetric, asymmetric, hashing, MAC)",
            "PKI, certificate lifecycle, key management",
            "common cryptographic attacks (replay, birthday, side-channel, padding)",
            "secure system architecture (rings, layering, abstraction)",
            "vulnerabilities in mobile / IoT / embedded / ICS-SCADA",
            "physical & environmental security (CPTED, fire suppression, HVAC)",
            "virtualization, containers, and cloud architecture risks",
        ],
        4 => &[
            "OSI vs TCP/IP model layer responsibilities",
            "secure protocols (TLS 1.3, IPsec AH/ESP, SSH, DNSSEC, S/MIME)",
            "network segmentation (VLAN, subnets, microsegmentation, zero trust)",
            "firewalls (stateful, NGFW), IDS/IPS, WAF placement",
            "VPN topologies (site-to-site, remote access, split tunnel)",
            "wireless security (WPA3, EAP variants, rogue AP)",
            "DDoS mitigation & egress filtering",
            "network attacks (ARP poisoning, DNS poisoning, session hijack, MITM)",
            "converged protocols (FCoE, iSCSI, VoIP/SIP) and risks",
            "CDN, SD-WAN, SASE concepts",
        ],
        5 => &[
            "identification, authentication, authorization, accountability (IAAA)",
            "authentication factors and MFA design",
            "federation (SAML, OIDC, OAuth2, WS-Fed)",
            "SSO, Kerberos, LDAP",
            "access control models (DAC, MAC, RBAC, ABAC, RuBAC)",
            "privileged access management (PAM), JIT, break-glass",
            "identity lifecycle (provision, recertify, deprovision)",
            "directory services and trust relationships",
            "biometrics (FAR, FRR, CER, enrollment)",
            "session management & token security",
        ],
        6 => &[
            "audit strategies (internal, external, third-party)",
            "control assessment vs control testing",
            "vulnerability management lifecycle",
            "penetration testing types (black/grey/white box, red/blue/purple team)",
            "SOC reports (SOC 1 / 2 / 3, Type I vs Type II)",
            "log management & SIEM strategy",
            "KPIs vs KRIs vs metrics",
            "code review (static, dynamic, IAST, fuzzing)",
            "disaster recovery testing (read-through, walkthrough, simulation, parallel, full interruption)",
            "continuous monitoring (NIST SP 800-137)",
        ],
        7 => &[
            "investigation types (admin, criminal, civil, regulatory)",
            "chain of custody, evidence handling, forensics order of volatility",
            "incident response phases (NIST SP 800-61)",
            "detection engineering, threat hunting",
            "backup strategies (full, differential, incremental, 3-2-1)",
            "DR strategies (hot/warm/cold/mobile/cloud sites)",
            "patch & change management",
            "configuration baselines, hardening, CIS benchmarks",
            "physical security operations (badging, mantraps, visitor mgmt)",
            "egress filtering, DLP operations, insider-threat program",
        ],
        8 => &[
            "SDLC phases & security activities at each phase",
            "secure coding (input validation, output encoding, parameterized queries)",
            "OWASP Top 10 (current) and ASVS",
            "threat modeling (STRIDE, DREAD, PASTA, attack trees)",
            "static vs dynamic vs interactive testing",
            "DevSecOps, CI/CD pipeline security",
            "software supply chain (SBOM, signing, dependency mgmt)",
            "API security (REST, GraphQL, authn/authz)",
            "database security (views, polyinstantiation, ACID)",
            "maturity models (CMMI, BSIMM, SAMM)",
        ],
        _ => &[],
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DomainStat {
    pub attempted: u32,
    pub correct: u32,
    pub recent_correct: Vec<u8>, // 0/1
}

impl DomainStat {
    pub fn rolling_accuracy(&self) -> f64 {
        if self.attempted < 5 {
            return 0.5; // cold-start
        }
        if self.recent_correct.is_empty() {
            return if self.attempted > 0 {
                self.correct as f64 / self.attempted as f64
            } else {
                0.5
            };
        }
        let sum: u32 = self.recent_correct.iter().map(|&x| x as u32).sum();
        sum as f64 / self.recent_correct.len() as f64
    }

    pub fn lifetime_accuracy(&self) -> Option<f64> {
        if self.attempted == 0 {
            None
        } else {
            Some(self.correct as f64 / self.attempted as f64)
        }
    }
}

pub type Stats = BTreeMap<u8, DomainStat>;
pub type Difficulty = BTreeMap<u8, u8>;

#[derive(Debug, Clone, Serialize)]
pub struct DomainPlan {
    pub domain: u8,
    pub tier: u8,
    pub count: u32,
    pub stretch: Option<StretchPlan>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StretchPlan {
    pub tier: u8,
    pub count: u32,
}

impl DomainPlan {
    pub fn total(&self) -> u32 {
        self.count + self.stretch.as_ref().map(|s| s.count).unwrap_or(0)
    }
}

/// Compute per-domain question counts for the next batch.
pub fn compute_distribution(stats: &Stats, diff: &Difficulty, target: u32) -> BTreeMap<u8, u32> {
    let mut weights = BTreeMap::new();
    let mut raw = BTreeMap::new();

    for (d, _, _) in DOMAINS {
        let stat = stats.get(&d).cloned().unwrap_or_default();
        let acc = stat.rolling_accuracy();
        let mut w = (1.0 - acc).powf(1.5) + 0.10;
        // mastery sanity: ≥85% rolling at top tier → minimal coverage
        if acc >= 0.85 && diff.get(&d).copied().unwrap_or(1) == 4 {
            w = 0.001;
        }
        weights.insert(d, w);
    }

    let sum_w: f64 = weights.values().sum::<f64>().max(f64::EPSILON);
    for (d, _, _) in DOMAINS {
        let w = *weights.get(&d).unwrap_or(&0.0);
        raw.insert(d, (w / sum_w) * target as f64);
    }

    // Initial round + clamp [floor, ceil].
    let mut dist: BTreeMap<u8, u32> = BTreeMap::new();
    for (d, _, _) in DOMAINS {
        let r = *raw.get(&d).unwrap_or(&0.0);
        let v = r.round() as i64;
        let clamped = v.clamp(COUNT_FLOOR as i64, COUNT_CEIL as i64) as u32;
        dist.insert(d, clamped);
    }

    // Drift fix to hit `target`.
    let mut total: i64 = dist.values().map(|&v| v as i64).sum();
    let mut safety = 200;
    let target = target as i64;
    while total != target && safety > 0 {
        safety -= 1;
        let dir: i64 = if total < target { 1 } else { -1 };
        let mut best_d: Option<u8> = None;
        let mut best_score = if dir > 0 { f64::NEG_INFINITY } else { f64::INFINITY };
        for (d, _, _) in DOMAINS {
            let v = *dist.get(&d).unwrap_or(&0);
            if dir > 0 && v >= COUNT_CEIL {
                continue;
            }
            if dir < 0 && v <= COUNT_FLOOR {
                continue;
            }
            let score = raw.get(&d).copied().unwrap_or(0.0) - v as f64;
            let take = if dir > 0 { score > best_score } else { score < best_score };
            if take {
                best_score = score;
                best_d = Some(d);
            }
        }
        match best_d {
            Some(d) => {
                let entry = dist.entry(d).or_insert(0);
                *entry = (*entry as i64 + dir) as u32;
                total += dir;
            }
            None => break,
        }
    }

    dist
}

/// Build the per-domain plan for a batch, including stretch questions when a
/// domain has ≥6 slots and isn't already at the top tier.
pub fn build_batch_plan(stats: &Stats, diff: &Difficulty, target: u32) -> Vec<DomainPlan> {
    let dist = compute_distribution(stats, diff, target);
    let mut out = Vec::with_capacity(8);
    for (d, _, _) in DOMAINS {
        let tier = diff.get(&d).copied().unwrap_or(1).clamp(1, 4);
        let total = *dist.get(&d).unwrap_or(&0);
        let mut count = total;
        let mut stretch = None;
        if total >= 6 && tier < 4 {
            let sc = ((total as f64 * 0.2).round() as u32).max(1);
            stretch = Some(StretchPlan { tier: tier + 1, count: sc });
            count = total.saturating_sub(sc);
        }
        out.push(DomainPlan { domain: d, tier, count, stretch });
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct TierChange {
    pub from: u8,
    pub to: u8,
}

/// Returns per-domain tier changes; mutates `diff` in place.
pub fn apply_difficulty_progression(stats: &Stats, diff: &mut Difficulty) -> BTreeMap<u8, TierChange> {
    let mut out = BTreeMap::new();
    for (d, _, _) in DOMAINS {
        let cur = diff.get(&d).copied().unwrap_or(1).clamp(1, 4);
        let s = stats.get(&d).cloned().unwrap_or_default();
        if s.recent_correct.len() < 5 {
            out.insert(d, TierChange { from: cur, to: cur });
            continue;
        }
        let acc = s.rolling_accuracy();
        let mut next = cur;
        if acc >= PROMOTE_THRESHOLD && cur < 4 {
            next = cur + 1;
        } else if acc <= DEMOTE_THRESHOLD && cur > 1 {
            next = cur - 1;
        }
        diff.insert(d, next);
        out.insert(d, TierChange { from: cur, to: next });
    }
    out
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_diff() -> Difficulty {
        let mut m = Difficulty::new();
        for (d, _, _) in DOMAINS {
            m.insert(d, 1);
        }
        m
    }

    fn empty_stats() -> Stats {
        let mut m = Stats::new();
        for (d, _, _) in DOMAINS {
            m.insert(d, DomainStat::default());
        }
        m
    }

    fn stat_with(attempted: u32, correct: u32, recent: &[u8]) -> DomainStat {
        DomainStat {
            attempted,
            correct,
            recent_correct: recent.to_vec(),
        }
    }

    #[test]
    fn rolling_accuracy_cold_start_returns_half() {
        let s = stat_with(2, 2, &[1, 1]);
        assert!((s.rolling_accuracy() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn rolling_accuracy_uses_window_when_warm() {
        let s = stat_with(20, 20, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // 1/10
        assert!((s.rolling_accuracy() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn rolling_accuracy_falls_back_to_lifetime_when_window_empty() {
        let s = stat_with(10, 4, &[]);
        assert!((s.rolling_accuracy() - 0.4).abs() < 1e-9);
    }

    #[test]
    fn distribution_sums_to_target_with_clamps_cold() {
        let stats = empty_stats();
        let diff = empty_diff();
        let dist = compute_distribution(&stats, &diff, BATCH_SIZE);
        let total: u32 = dist.values().sum();
        assert_eq!(total, BATCH_SIZE);
        for (_, v) in &dist {
            assert!(*v >= COUNT_FLOOR);
            assert!(*v <= COUNT_CEIL);
        }
        assert_eq!(dist.len(), DOMAINS.len());
    }

    #[test]
    fn distribution_pulls_weight_to_weakest_domain() {
        let mut stats = empty_stats();
        // D5 weak (10% rolling) — fully attempted
        stats.insert(5, stat_with(20, 2, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        // Other 7 strong (90%)
        for (d, _, _) in DOMAINS {
            if d == 5 {
                continue;
            }
            stats.insert(d, stat_with(20, 18, &[1, 1, 1, 1, 1, 1, 1, 1, 1, 0]));
        }
        let diff = empty_diff();
        let dist = compute_distribution(&stats, &diff, BATCH_SIZE);
        let weak = *dist.get(&5).unwrap();
        for (d, _, _) in DOMAINS {
            if d == 5 {
                continue;
            }
            assert!(
                weak >= *dist.get(&d).unwrap(),
                "weak domain {} should have ≥ count than domain {}",
                5,
                d
            );
        }
        // Weak domain should be at or near the ceiling.
        assert!(weak >= 8, "expected weak domain to grow toward ceiling, got {weak}");
    }

    #[test]
    fn mastered_top_tier_domain_collapses_to_floor() {
        let mut stats = empty_stats();
        // D3 mastered: 95% rolling.
        stats.insert(3, stat_with(40, 38, &[1, 1, 1, 1, 1, 1, 1, 1, 1, 1]));
        // Everyone else neutral (50% rolling, attempted ≥ 5)
        for (d, _, _) in DOMAINS {
            if d == 3 {
                continue;
            }
            stats.insert(d, stat_with(10, 5, &[1, 0, 1, 0, 1, 0, 1, 0, 1, 0]));
        }
        let mut diff = empty_diff();
        diff.insert(3, 4); // top tier
        let dist = compute_distribution(&stats, &diff, BATCH_SIZE);
        assert_eq!(*dist.get(&3).unwrap(), COUNT_FLOOR);
    }

    #[test]
    fn build_batch_plan_adds_stretch_when_count_ge_6_and_below_top() {
        let mut stats = empty_stats();
        // Force D7 to be the weakest so it gets a fat slice.
        stats.insert(7, stat_with(20, 0, &[0; 10]));
        let mut diff = empty_diff();
        diff.insert(7, 2);
        let plan = build_batch_plan(&stats, &diff, BATCH_SIZE);
        let p7 = plan.iter().find(|p| p.domain == 7).unwrap();
        assert!(p7.count + p7.stretch.as_ref().map(|s| s.count).unwrap_or(0) >= 6);
        let s = p7.stretch.as_ref().expect("stretch should be present at tier 2 with ≥6 count");
        assert_eq!(s.tier, 3);
        assert!(s.count >= 1);
    }

    #[test]
    fn build_batch_plan_no_stretch_at_top_tier() {
        let mut stats = empty_stats();
        stats.insert(4, stat_with(20, 0, &[0; 10]));
        let mut diff = empty_diff();
        diff.insert(4, 4); // top tier — never stretch
        let plan = build_batch_plan(&stats, &diff, BATCH_SIZE);
        let p4 = plan.iter().find(|p| p.domain == 4).unwrap();
        assert!(p4.stretch.is_none());
    }

    #[test]
    fn build_batch_plan_no_stretch_when_count_under_6() {
        let stats = empty_stats(); // all cold-start, even slice ≈ 6.25 → most domains 6
        let diff = empty_diff();
        let plan = build_batch_plan(&stats, &diff, 24); // smaller target → ~3 each
        for p in &plan {
            // For target 24 nobody should breach 6.
            assert!(
                p.count + p.stretch.as_ref().map(|s| s.count).unwrap_or(0) < 6
                    || p.stretch.is_some(),
                "plan invariant violated for D{}",
                p.domain
            );
            if p.count + p.stretch.as_ref().map(|s| s.count).unwrap_or(0) < 6 {
                assert!(p.stretch.is_none(), "D{} got stretch under 6 slots", p.domain);
            }
        }
    }

    #[test]
    fn progression_promotes_at_or_above_threshold() {
        let mut stats = empty_stats();
        // 8/10 = 0.80, exactly the promote threshold.
        stats.insert(1, stat_with(20, 16, &[1, 1, 1, 1, 1, 1, 1, 1, 0, 0]));
        let mut diff = empty_diff();
        diff.insert(1, 2);
        let changes = apply_difficulty_progression(&stats, &mut diff);
        assert_eq!(diff.get(&1).copied().unwrap(), 3);
        let c = changes.get(&1).unwrap();
        assert_eq!(c.from, 2);
        assert_eq!(c.to, 3);
    }

    #[test]
    fn progression_demotes_at_or_below_threshold() {
        let mut stats = empty_stats();
        // 4/10 = 0.40, exactly the demote threshold.
        stats.insert(2, stat_with(20, 8, &[1, 1, 1, 1, 0, 0, 0, 0, 0, 0]));
        let mut diff = empty_diff();
        diff.insert(2, 3);
        apply_difficulty_progression(&stats, &mut diff);
        assert_eq!(diff.get(&2).copied().unwrap(), 2);
    }

    #[test]
    fn progression_holds_in_middle_band() {
        let mut stats = empty_stats();
        // 6/10 = 0.60, between thresholds.
        stats.insert(3, stat_with(20, 12, &[1, 1, 1, 1, 1, 1, 0, 0, 0, 0]));
        let mut diff = empty_diff();
        diff.insert(3, 2);
        apply_difficulty_progression(&stats, &mut diff);
        assert_eq!(diff.get(&3).copied().unwrap(), 2);
    }

    #[test]
    fn progression_skips_when_window_under_5() {
        let mut stats = empty_stats();
        // Perfect score but only 3 samples — should not promote.
        stats.insert(4, stat_with(3, 3, &[1, 1, 1]));
        let mut diff = empty_diff();
        diff.insert(4, 1);
        apply_difficulty_progression(&stats, &mut diff);
        assert_eq!(diff.get(&4).copied().unwrap(), 1);
    }

    #[test]
    fn progression_does_not_overshoot_top_or_bottom() {
        let mut stats = empty_stats();
        // Domain at top tier crushing it shouldn't go to tier 5.
        stats.insert(5, stat_with(20, 20, &[1; 10]));
        // Domain at bottom tier flubbing shouldn't go below 1.
        stats.insert(6, stat_with(20, 0, &[0; 10]));
        let mut diff = empty_diff();
        diff.insert(5, 4);
        diff.insert(6, 1);
        apply_difficulty_progression(&stats, &mut diff);
        assert_eq!(diff.get(&5).copied().unwrap(), 4);
        assert_eq!(diff.get(&6).copied().unwrap(), 1);
    }

    #[test]
    fn domain_anchors_cover_all_eight_domains() {
        for (d, _, _) in DOMAINS {
            let a = domain_anchors(d);
            assert!(!a.is_empty(), "domain {d} has no anchors");
            assert!(a.len() >= 5, "domain {d} should have ≥5 anchors, got {}", a.len());
        }
        assert!(domain_anchors(99).is_empty());
    }
}
