import sys
from pathlib import Path

ME = '''        for step in [16, 8, 4] {
            let origin = motions.integer.ok_or(Error::Internal)?.2;'''
PLANS = '''        let mut plans = [whole, split];'''
CANDIDATES = '''        let mut settled = false;
        let mut coded = None;'''
REFINE = '''        if candidate.inter.as_ref().is_some_and(|i| i.skip) {
            return Ok(candidate);
        }'''
RANK = '''
thread_local! {
    static INTER_RANK: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}
'''
I4_OLD = '''        for (slot, mode) in ranked.iter_mut().zip(0..9) {
            if !legal(mode, 4, a) {
                continue;
            }
            tile.prediction(x, y, 4, a, mode)?;
            let mut bits = Bits::count();
            mode_bits(&mut bits, mode, predicted)?;
            *slot = (
                tile.satd(source, x, y, 4)? + (1u64 << (qp / 6)) * bits.len as u64,
                mode,
            );
        }'''
I4_NEW = '''        let mut best_mode = usize::MAX;
        let mut round = [0u8, 1, 2, predicted];
        let mut count = 4;
        for _ in 0..3 {
            let mut improved = false;
            for &mode in round.iter().take(count) {
                let slot = ranked.get_mut(mode as usize).ok_or(Error::Internal)?;
                if *slot != UNRANKED || !legal(mode, 4, a) {
                    continue;
                }
                tile.prediction(x, y, 4, a, mode)?;
                let mut bits = Bits::count();
                mode_bits(&mut bits, mode, predicted)?;
                *slot = (
                    tile.satd(source, x, y, 4)? + (1u64 << (qp / 6)) * bits.len as u64,
                    mode,
                );
                let value = *slot;
                if best_mode == usize::MAX || value < ranked[best_mode] {
                    best_mode = mode as usize;
                    improved = true;
                }
            }
            if !improved {
                break;
            }
            let next: &[u8] = match best_mode {
                0 => &[5, 7],
                1 => &[6, 8],
                3 => &[7],
                4 => &[5, 6],
                5 => &[0, 4],
                6 => &[1, 4],
                7 => &[0, 3],
                8 => &[1],
                _ => &[],
            };
            count = next.len();
            round[..count].copy_from_slice(next);
        }'''

ARMS = {
    'skip-settled': [('inter.rs', '''        let ceiling = best.as_ref().map(|candidate| candidate.key.cost);
        let whole = self.propose(Layout::Whole, ceiling)?;''', '''        if settled {
            return Ok(IntraScope::Sixteen);
        }
        let ceiling = best.as_ref().map(|candidate| candidate.key.cost);
        let whole = self.propose(Layout::Whole, ceiling)?;''', 1)],
    'split-128': [('inter.rs', 'const SPLIT_SATD: u64 = 64;', 'const SPLIT_SATD: u64 = 128;', 1)],
    'split-256': [('inter.rs', 'const SPLIT_SATD: u64 = 64;', 'const SPLIT_SATD: u64 = 256;', 1)],
    'split-rank': [('inter.rs', PLANS, '''        let split = split.filter(|s| whole.as_ref().is_none_or(|w| s.2 < w.2));
''' + PLANS, 1)],
    'me-step8': [('inter.rs', ME, ME.replace('[16, 8, 4]', '[8, 4]'), 1)],
    'me-early': [('inter.rs', ME, '''        let early = motions.integer.ok_or(Error::Internal)?.0
            <= (part.width * part.height) as u64 * (1u64 << (self.decision.qp / 6)) / 4;
        for step in [16, 8, 4] {
            if early {
                break;
            }
            let origin = motions.integer.ok_or(Error::Internal)?.2;''', 1)],
    'me-rescore1': [('inter.rs', '            motions.ranked.get(1).copied().flatten(),\n', '            None,\n', 1)],
    'me-noqpel': [('inter.rs', '''        let mut centre = motions.integer.ok_or(Error::Internal)?;
        for step in [2, 1] {''', '''        let mut centre = motions.integer.ok_or(Error::Internal)?;
        for step in [2] {''', 2)],
    'i4-dirs': [('codec.rs', I4_OLD, I4_NEW, 1)],
    'i4-gate': [
        ('codec.rs', '''        let four = if four {
            Some(intra4(''', '''        let four = if four && INTER_RANK.with(|c| c.get()).is_none_or(|rank| rank > luma.0) {
            Some(intra4(''', 1),
        ('codec.rs', None, RANK, 0),
        ('inter.rs', CANDIDATES, '''        INTER_RANK.with(|c| c.set(None));
''' + CANDIDATES, 1),
        ('inter.rs', PLANS, '''        INTER_RANK.with(|c| c.set([whole.as_ref(), split.as_ref()].into_iter().flatten().map(|p| p.2).min()));
''' + PLANS, 1),
    ],
    'rd-sparse': [('levels.rs', REFINE, REFINE + '''
        if candidate.inter.is_some()
            && candidate
                .luma
                .levels
                .iter()
                .flatten()
                .chain(candidate.chroma.levels.iter().flatten().flatten())
                .chain(candidate.luma.dc.iter())
                .chain(candidate.chroma.dc.iter().flatten())
                .filter(|&&level| level != 0)
                .count()
                <= 4
        {
            return Ok(candidate);
        }''', 1)],
    'rd-pinter': [('levels.rs', REFINE, REFINE + '''
        if candidate.inter.is_some() {
            return Ok(candidate);
        }''', 1)],
}


PROPOSE = '    fn propose(&self, layout: Layout, ceiling: Option<u64>) -> Result<Option<(usize, Inter, u64)>> {'
SPLIT_CALL = '            self.propose(Layout::Sub([0; 4]), ceiling)?'


def abort(margin):
    return [
        ('inter.rs', PROPOSE, PROPOSE.replace('ceiling: Option<u64>)', 'ceiling: Option<u64>, budget: Option<u64>)'), 1),
        ('inter.rs', '            rank += cost;\n', '            rank += cost;\n            if budget.is_some_and(|b| rank >= b) {\n                return Ok(None);\n            }\n', 1),
        ('inter.rs', '        let whole = self.propose(Layout::Whole, ceiling)?;', '        let whole = self.propose(Layout::Whole, ceiling, None)?;', 1),
        ('inter.rs', SPLIT_CALL, f'            self.propose(Layout::Sub([0; 4]), ceiling, whole.as_ref().map(|w| w.2 + w.2 * {margin} / 8))?', 1),
    ]


ARMS.update({
    'split-80': [('inter.rs', 'const SPLIT_SATD: u64 = 64;', 'const SPLIT_SATD: u64 = 80;', 1)],
    'split-96': [('inter.rs', 'const SPLIT_SATD: u64 = 64;', 'const SPLIT_SATD: u64 = 96;', 1)],
    'split-abort': abort(0),
    'split-abort-8': abort(1),
    'split-abort-4': abort(2),
    'settled-nosplit': [('inter.rs', '        let split = if whole\n', '        let split = if !settled && whole\n', 1)],
    'i4-gate2': [(f, o, n.replace('rank > luma.0', 'rank * 2 > luma.0'), c) for f, o, n, c in ARMS['i4-gate']],
    'i4-dirs-p': [('codec.rs', I4_OLD, '        if trials == Trials::Cheap {\n' + I4_NEW + '\n        } else {\n' + I4_OLD + '\n        }', 1)],
})


SPLIT_TEST = '            .is_none_or(|plan| plan.2 > SPLIT_SATD << (self.decision.qp / 6))'
ARMS.update({
    'settled-split2': [('inter.rs', SPLIT_TEST, SPLIT_TEST.replace('SPLIT_SATD <<', '(SPLIT_SATD * if settled { 2 } else { 1 }) <<'), 1)],
    'settled-split4': [('inter.rs', SPLIT_TEST, SPLIT_TEST.replace('SPLIT_SATD <<', '(SPLIT_SATD * if settled { 4 } else { 1 }) <<'), 1)],
    'i4-gate4': [(f, o, n.replace('rank > luma.0', 'rank * 4 > luma.0'), c) for f, o, n, c in ARMS['i4-gate']],
    'i4-gate8': [(f, o, n.replace('rank > luma.0', 'rank * 8 > luma.0'), c) for f, o, n, c in ARMS['i4-gate']],
})


Q_OLD = '''    fn quiet(&self, skip: &Candidate) -> Result<bool> {
        let quant = t::Quant::of(self.decision.qp)?;
        for k in 0..16 {
            let (x, y) = xy(k);
            if !quant.zero(&skip.luma.tile.forward(self.decision.source, x, y)?, false) {
                return Ok(false);
            }
        }
        let qp = t::chroma_qp(self.decision.qp);'''
Q_NEW = '''    fn luma_quiet(&self, skip: &Candidate) -> Result<bool> {
        let quant = t::Quant::of(self.decision.qp)?;
        for k in 0..16 {
            let (x, y) = xy(k);
            if !quant.zero(&skip.luma.tile.forward(self.decision.source, x, y)?, false) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn chroma_quiet(&self, skip: &Candidate) -> Result<bool> {
        let qp = t::chroma_qp(self.decision.qp);'''
SKIP_EVAL = '''                    Ok(c) => {
                        let quiet = self.quiet(&c)?;
                        let cost = c.key.cost;'''
SETTLE = '''                                settled = c.key.cost >= cost;'''
BEFORE_SEARCH = '''        let ceiling = best.as_ref().map(|candidate| candidate.key.cost);
        let whole = self.propose(Layout::Whole, ceiling)?;'''
EARLY_EXIT = '''        if early {
            return Ok(IntraScope::Sixteen);
        }
''' + BEFORE_SEARCH
ARMS.update({
    'luma-quiet': [
        ('inter.rs', Q_OLD, Q_NEW, 1),
        ('inter.rs', CANDIDATES, CANDIDATES + '\n        let mut early = false;', 1),
        ('inter.rs', SKIP_EVAL, SKIP_EVAL.replace('let quiet = self.quiet(&c)?;', 'let luma_quiet = self.luma_quiet(&c)?;\n                        let quiet = luma_quiet && self.chroma_quiet(&c)?;'), 1),
        ('inter.rs', SETTLE, SETTLE + '\n                                early = settled && luma_quiet;', 1),
        ('inter.rs', BEFORE_SEARCH, EARLY_EXIT, 1),
    ],
    'settled-satd': [
        ('inter.rs', CANDIDATES, CANDIDATES + '\n        let mut early = false;', 1),
        ('inter.rs', SKIP_EVAL, SKIP_EVAL + '\n                        let near = c.luma.tile.satd(self.decision.source, 0, 0, 16)? <= SPLIT_SATD << (self.decision.qp / 6);', 1),
        ('inter.rs', SETTLE, SETTLE + '\n                                early = settled && near;', 1),
        ('inter.rs', BEFORE_SEARCH, EARLY_EXIT, 1),
    ],
})


ARMS['pair'] = ARMS['settled-split2'] + ARMS['i4-gate4']


def apply(tree, arm):
    src = Path(tree) / 'crates/coreav-h264-encoder/src'
    for name, old, new, count in ARMS[arm]:
        path = src / name
        text = path.read_text()
        if old is None:
            text += new
        else:
            found = text.count(old)
            assert found == count, (arm, name, found, count)
            text = text.replace(old, new)
        path.write_text(text)


if __name__ == '__main__':
    apply(sys.argv[1], sys.argv[2])
