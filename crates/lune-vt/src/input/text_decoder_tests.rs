//! Port of xterm.js src/common/input/TextDecoder.test.ts @ f447274 (the `Utf8ToUtf32` and
//! `stringFromCodePoint` / `utf32ToString` tests), plus the ledger-3 split-sequence tests and
//! Rust-only no-panic cases.
//!
//! Not ported: every `StringToUtf32` test ("replaced": Rust input is bytes, plan D5). Upstream's
//! stream tests end in `assert(decoded, ...)`, which is vacuously true; here they compare.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

const TEST_STRINGS: [&str; 9] = [
    "Лорем ипсум долор сит амет, ех сеа аццусам диссентиет. Ан еос стет еирмод витуперата. Иус дицерет урбанитас ет. Ан при алтера долорес сплендиде, цу яуо интегре денияуе, игнота волуптариа инструцтиор цу вим.",
    "ლორემ იფსუმ დოლორ სით ამეთ, ფაცერ მუციუს ცონსეთეთურ ყუო იდ, ფერ ვივენდუმ ყუაერენდუმ ეა, ესთ ამეთ მოვეთ სუავითათე ცუ. ვითაე სენსიბუს ან ვიხ. ეხერცი დეთერრუისსეთ უთ ყუი. ვოცენთ დებითის ადიფისცი ეთ ფერ. ნეც ან ფეუგაით ფორენსიბუს ინთერესსეთ. იდ დიცო რიდენს იუს. დისსენთიეთ ცონსეყუუნთურ სედ ნე, ნოვუმ მუნერე ეუმ ათ, ნე ეუმ ნიჰილ ირაცუნდია ურბანითას.",
    "अधिकांश अमितकुमार प्रोत्साहित मुख्य जाने प्रसारन विश्लेषण विश्व दारी अनुवादक अधिकांश नवंबर विषय गटकउसि गोपनीयता विकास जनित परस्पर गटकउसि अन्तरराष्ट्रीयकरन होसके मानव पुर्णता कम्प्युटर यन्त्रालय प्रति साधन",
    "覧六子当聞社計文護行情投身斗来。増落世的況上席備界先関権能万。本物挙歯乳全事携供板栃果以。頭月患端撤競見界記引去法条公泊候。決海備駆取品目芸方用朝示上用報。講申務紙約週堂出応理田流団幸稿。起保帯吉対阜庭支肯豪彰属本躍。量抑熊事府募動極都掲仮読岸。自続工就断庫指北速配鳴約事新住米信中験。婚浜袋著金市生交保他取情距。",
    "八メル務問へふらく博辞説いわょ読全タヨムケ東校どっ知壁テケ禁去フミ人過を装5階がねぜ法逆はじ端40落ミ予竹マヘナセ任1悪た。省ぜりせ製暇ょへそけ風井イ劣手はぼまず郵富法く作断タオイ取座ゅょが出作ホシ月給26島ツチ皇面ユトクイ暮犯リワナヤ断連こうでつ蔭柔薄とレにの。演めけふぱ損田転10得観びトげぎ王物鉄夜がまけ理惜くち牡提づ車惑参ヘカユモ長臓超漫ぼドかわ。",
    "모든 국민은 행위시의 법률에 의하여 범죄를 구성하지 아니하는 행위로 소추되지 아니하며. 전직대통령의 신분과 예우에 관하여는 법률로 정한다, 국회는 헌법 또는 법률에 특별한 규정이 없는 한 재적의원 과반수의 출석과 출석의원 과반수의 찬성으로 의결한다. 군인·군무원·경찰공무원 기타 법률이 정하는 자가 전투·훈련등 직무집행과 관련하여 받은 손해에 대하여는 법률이 정하는 보상외에 국가 또는 공공단체에 공무원의 직무상 불법행위로 인한 배상은 청구할 수 없다.",
    "كان فشكّل الشرقي مع, واحدة للمجهود تزامناً بعض بل. وتم جنوب للصين غينيا لم, ان وبدون وكسبت الأمور ذلك, أسر الخاسر الانجليزية هو. نفس لغزو مواقعها هو. الجو علاقة الصعداء انه أي, كما مع بمباركة للإتحاد الوزراء. ترتيب الأولى أن حدى, الشتوية باستحداث مدن بل, كان قد أوسع عملية. الأوضاع بالمطالبة كل قام, دون إذ شمال الربيع،. هُزم الخاصّة ٣٠ أما, مايو الصينية مع قبل.",
    "או סדר החול מיזמי קרימינולוגיה. קהילה בגרסה לויקיפדים אל היא, של צעד ציור ואלקטרוניקה. מדע מה ברית המזנון ארכיאולוגיה, אל טבלאות מבוקשים כלל. מאמרשיחהצפה העריכהגירסאות שכל אל, כתב עיצוב מושגי של. קבלו קלאסיים ב מתן. נבחרים אווירונאוטיקה אם מלא, לוח למנוע ארכיאולוגיה מה. ארץ לערוך בקרבת מונחונים או, עזרה רקטות לויקיפדים אחר גם.",
    "Лорем ლორემ अधिकांश 覧六子 八メル 모든 בקרבת 💮 😂 äggg 123€ 𝄞.",
];

/// Decode `input` in one call.
fn decode_whole(input: &[u8]) -> (Vec<u32>, Vec<u8>) {
    let mut d = Utf8ToUtf32::new();
    let mut target = Vec::new();
    let n = d.decode(input, &mut target);
    assert_eq!(n, target.len());
    (target, d.interim().to_vec())
}

/// Decode `input` cut at the given (sorted, in-range) offsets; concatenate the outputs.
fn decode_split(input: &[u8], cuts: &[usize]) -> (Vec<u32>, Vec<u8>) {
    let mut d = Utf8ToUtf32::new();
    let mut out = Vec::new();
    let mut target = Vec::new();
    let mut prev = 0;
    for &c in cuts.iter().chain(std::iter::once(&input.len())) {
        let n = d.decode(&input[prev..c], &mut target);
        assert_eq!(n, target.len());
        out.extend_from_slice(&target);
        prev = c;
    }
    (out, d.interim().to_vec())
}

fn decode_one_byte_at_a_time(input: &[u8]) -> (Vec<u32>, Vec<u8>) {
    let cuts: Vec<usize> = (1..input.len()).collect();
    decode_split(input, &cuts)
}

fn encode(cp: u32) -> Vec<u8> {
    let mut buf = [0u8; 4];
    char::from_u32(cp)
        .unwrap()
        .encode_utf8(&mut buf)
        .as_bytes()
        .to_vec()
}

fn to_string(data: &[u32]) -> String {
    data.iter().map(|&c| char::from_u32(c).unwrap()).collect()
}

mod text_encodings {
    use super::*;

    #[test]
    fn string_from_code_point_and_utf32_to_string() {
        let s = "abcdefg";
        let data: Vec<u32> = s.chars().map(u32::from).collect();
        for (i, c) in s.chars().enumerate() {
            assert_eq!(string_from_code_point(data[i]), c.to_string());
        }
        assert_eq!(utf32_to_string(&data), s);
        assert_eq!(
            utf32_to_string(&[0x1F600, 0x10FFFF, 0]),
            "\u{1F600}\u{10FFFF}\0"
        );
        assert_eq!(string_from_code_point(0x1D11E), "\u{1D11E}");
    }

    /// LUNETERM: Rust strings cannot hold surrogates or values above U+10FFFF.
    #[test]
    fn utf32_to_string_never_panics_on_non_scalars() {
        assert_eq!(
            utf32_to_string(&[0xD800, 0x41, 0xDFFF]),
            "\u{FFFD}A\u{FFFD}"
        );
        assert_eq!(utf32_to_string(&[0x11_0000, u32::MAX]), "\u{FFFD}\u{FFFD}");
        assert_eq!(string_from_code_point(0xD800), "\u{FFFD}");
        assert_eq!(string_from_code_point(u32::MAX), "\u{FFFD}");
        assert_eq!(utf32_to_string(&[]), "");
    }
}

mod utf8_to_utf32_decoder {
    use super::*;

    const BATCH_SIZE: u32 = 2048;

    mod full_codepoint_test {
        use super::*;

        /// Upstream: one `it` per 2048-code-point batch of 0..65535 (1/2/3 byte sequences).
        #[test]
        fn one_two_and_three_byte_sequences() {
            let mut d = Utf8ToUtf32::new();
            let mut target = Vec::new();
            let mut min = 0;
            while min < 65535 {
                let max = (min + BATCH_SIZE).min(65536);
                for i in min..max {
                    // skip surrogate pairs and a BOM
                    if (0xD800..=0xDFFF).contains(&i) || i == 0xFEFF {
                        continue;
                    }
                    let length = d.decode(&encode(i), &mut target);
                    assert_eq!(length, 1, "U+{i:04X}");
                    assert_eq!(to_string(&target), char::from_u32(i).unwrap().to_string());
                    d.reset();
                }
                min += BATCH_SIZE;
            }
        }

        /// Upstream stops short of U+10FFFF (`max = min(..., 0x10FFFF)` is exclusive); this
        /// includes it.
        #[test]
        fn four_byte_sequences() {
            let mut d = Utf8ToUtf32::new();
            let mut target = Vec::new();
            for i in 65536..=0x10FFFF {
                let length = d.decode(&encode(i), &mut target);
                assert_eq!(length, 1, "U+{i:04X}");
                assert_eq!(target[0], i);
                d.reset();
            }
        }

        #[test]
        fn bom() {
            let mut d = Utf8ToUtf32::new();
            let mut target = Vec::new();
            let length = d.decode(&encode(0xFEFF), &mut target);
            assert_eq!(length, 0);
            d.reset();
        }
    }

    #[test]
    fn test_strings() {
        let mut d = Utf8ToUtf32::new();
        let mut target = Vec::new();
        for s in TEST_STRINGS {
            let length = d.decode(s.as_bytes(), &mut target);
            assert_eq!(length, target.len());
            assert_eq!(to_string(&target), s);
            d.reset();
        }
    }

    mod stream_handling {
        use super::*;

        const TWO_BYTE: &str = "\u{c4}\u{d6}\u{dc}\u{df}\u{f6}\u{e4}\u{fc}";
        const TWO_THREE: &str = "\u{c4}\u{20ac}\u{d6}\u{20ac}\u{dc}\u{20ac}\u{df}\u{20ac}\u{f6}\u{20ac}\u{e4}\u{20ac}\u{fc}";
        const MIXED: &str = "\u{c4}\u{20ac}\u{1d11e}\u{d6}\u{1d11e}\u{20ac}\u{dc}\u{1d11e}\u{20ac}";

        fn advance_by(text: &str, step: usize) -> String {
            let bytes = text.as_bytes();
            let mut d = Utf8ToUtf32::new();
            let mut target = Vec::new();
            let mut decoded = String::new();
            let mut i = 0;
            while i < bytes.len() {
                let end = (i + step).min(bytes.len());
                d.decode(&bytes[i..end], &mut target);
                decoded += &to_string(&target);
                i += step;
            }
            decoded
        }

        #[test]
        fn two_byte_sequences_advance_by_1() {
            assert_eq!(advance_by(TWO_BYTE, 1), TWO_BYTE);
        }

        #[test]
        fn two_three_byte_sequences_advance_by_1() {
            assert_eq!(advance_by(TWO_THREE, 1), TWO_THREE);
        }

        #[test]
        fn two_three_four_byte_sequences_advance_by_1() {
            assert_eq!(advance_by(MIXED, 1), MIXED);
        }

        #[test]
        fn two_three_four_byte_sequences_advance_by_2() {
            assert_eq!(advance_by(MIXED, 2), MIXED);
        }

        #[test]
        fn two_three_four_byte_sequences_advance_by_3() {
            assert_eq!(advance_by(MIXED, 3), MIXED);
        }

        #[test]
        fn boms_three_byte_sequences_advance_by_2() {
            let bytes = [0xef, 0xbb, 0xbf, 0xef, 0xbb, 0xbf];
            let mut d = Utf8ToUtf32::new();
            let mut target = Vec::new();
            let mut decoded = String::new();
            for chunk in bytes.chunks(2) {
                d.decode(chunk, &mut target);
                decoded += &to_string(&target);
            }
            assert_eq!(decoded, "");
        }

        /// issue #2495
        #[test]
        fn break_after_3_bytes() {
            let bytes = [0xf0, 0xa0, 0x9c, 0x8e];
            let mut d = Utf8ToUtf32::new();
            let mut target = Vec::new();
            let mut written = d.decode(&bytes[..3], &mut target);
            assert_eq!(written, 0);
            written = d.decode(&bytes[3..], &mut target);
            assert_eq!(written, 1);
            assert_eq!(to_string(&target), "\u{2070e}");
        }
    }
}

// ---- Ledger 3: chunk boundaries must not change the decoded output (D4) ----

/// One scalar of each encoded length class and the boundaries between them.
const SCALARS: [u32; 15] = [
    0x41, 0x7F, 0x80, 0x7FF, 0x800, 0x2019, 0xFFFD, 0xFFFF, 0x1_0000, 0x1_F600, 0x10_FFFF, 0x20AC,
    0x3000, 0xE000, 0x1_D11E,
];

#[test]
fn split_utf8_every_scalar_class_at_every_byte_boundary() {
    for cp in SCALARS {
        let bytes = encode(cp);
        for cut in 0..=bytes.len() {
            let (out, interim) = decode_split(&bytes, &[cut]);
            assert_eq!(out, [cp], "U+{cp:04X} cut at {cut}");
            assert!(interim.is_empty(), "U+{cp:04X} cut at {cut}");
        }
    }
}

#[test]
fn split_utf8_every_scalar_class_one_byte_per_call() {
    for cp in SCALARS {
        let bytes = encode(cp);
        let (out, interim) = decode_one_byte_at_a_time(&bytes);
        assert_eq!(out, [cp], "U+{cp:04X}");
        assert!(interim.is_empty());
    }
}

#[test]
fn split_utf8_every_scalar_in_context_at_every_split_pair() {
    // Neighbours on both sides, then every pair of cut points.
    for cp in SCALARS {
        let mut bytes = b"a".to_vec();
        bytes.extend(encode(cp));
        bytes.extend(encode(0x2019));
        bytes.push(b'z');
        let expected = vec![0x61, cp, 0x2019, 0x7A];
        let n = bytes.len();
        for c1 in 0..=n {
            for c2 in c1..=n {
                let (out, _) = decode_split(&bytes, &[c1, c2]);
                assert_eq!(out, expected, "U+{cp:04X} cuts {c1},{c2}");
            }
        }
    }
}

/// The reported bug: `E2 80 | 99 41` lost U+2019 (and the `A`).
#[test]
fn split_utf8_after_80_continuation() {
    let mut d = Utf8ToUtf32::new();
    let mut target = Vec::new();
    assert_eq!(d.decode(&[0xE2, 0x80], &mut target), 0);
    assert_eq!(d.interim(), [0xE2, 0x80]);
    assert_eq!(d.decode(&[0x99, 0x41], &mut target), 2);
    assert_eq!(target, [0x2019, 0x41]);
    assert!(d.interim().is_empty());
}

/// Other continuation bytes whose low six bits are zero: 0x80 in either interim slot.
#[test]
fn split_utf8_after_80_in_every_interim_position() {
    // U+0800 = E0 A0 80, U+10000 = F0 90 80 80, U+C0000 = F3 80 80 80
    for (bytes, cp) in [
        (vec![0xE0, 0xA0, 0x80], 0x800),
        (vec![0xF0, 0x90, 0x80, 0x80], 0x1_0000),
        (vec![0xF3, 0x80, 0x80, 0x80], 0xC_0000),
        (vec![0xE2, 0x80, 0x80], 0x2000),
    ] {
        for cut in 0..=bytes.len() {
            let mut input = bytes.clone();
            input.extend([0x41, 0x42]);
            let (out, _) = decode_split(&input, &[cut]);
            assert_eq!(out, [cp, 0x41, 0x42], "{bytes:02X?} cut {cut}");
        }
        let mut input = bytes.clone();
        input.extend([0x41, 0x42]);
        let (out, _) = decode_one_byte_at_a_time(&input);
        assert_eq!(out, [cp, 0x41, 0x42], "{bytes:02X?} one byte per call");
    }
}

#[test]
fn split_utf8_interim_is_readable_and_reset_clears_it() {
    let mut d = Utf8ToUtf32::new();
    let mut target = Vec::new();
    d.decode(&[0x61, 0xF0, 0x90, 0x80], &mut target);
    assert_eq!(target, [0x61]);
    assert_eq!(d.interim(), [0xF0, 0x90, 0x80]);
    d.reset();
    assert!(d.interim().is_empty());
    assert_eq!(d.decode(&[0x80], &mut target), 0);
}

#[test]
fn split_utf8_bad_continuation_after_interim_is_reread() {
    // E2 80 | 41: the lead and 0x80 are dropped, 'A' survives.
    let mut d = Utf8ToUtf32::new();
    let mut target = Vec::new();
    d.decode(&[0xE2, 0x80], &mut target);
    assert_eq!(d.decode(&[0x41], &mut target), 1);
    assert_eq!(target, [0x41]);
    assert!(d.interim().is_empty());
}

#[test]
fn split_utf8_empty_chunks_change_nothing() {
    let (out, _) = decode_split(&[0xE2, 0x80, 0x99, 0x41], &[0, 1, 1, 2, 2, 2, 4]);
    assert_eq!(out, [0x2019, 0x41]);
}

/// xorshift64*; seeded, no dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Random byte string biased to UTF-8 structure, including invalid lead/continuation mixes.
fn random_bytes(rng: &mut Rng) -> Vec<u8> {
    let len = rng.below(41);
    (0..len)
        .map(|_| match rng.below(10) {
            0 | 1 => rng.below(0x80) as u8,        // ASCII
            2..=4 => 0x80 + rng.below(0x40) as u8, // continuation
            5 => 0x80,                             // the sentinel byte
            6 => 0xC0 + rng.below(0x20) as u8,     // 2-byte leads (incl. overlong C0/C1)
            7 => 0xE0 + rng.below(0x10) as u8,     // 3-byte leads
            8 => 0xF0 + rng.below(0x08) as u8,     // 4-byte leads (incl. > F4)
            _ => 0xF8 + rng.below(0x08) as u8,     // invalid F8..FF
        })
        .collect()
}

/// Property: every random byte string, split at random cut points (empty chunks allowed),
/// decodes to the same code points and ends with the same held-back bytes as one call.
#[test]
fn split_utf8_property_random_splits_match_whole_buffer() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for case in 0..2000 {
        let bytes = random_bytes(&mut rng);
        let whole = decode_whole(&bytes);
        for _ in 0..8 {
            let n_cuts = rng.below(6);
            let mut cuts: Vec<usize> = (0..n_cuts).map(|_| rng.below(bytes.len() + 1)).collect();
            cuts.sort_unstable();
            assert_eq!(
                decode_split(&bytes, &cuts),
                whole,
                "case {case}: {bytes:02X?} cuts {cuts:?}"
            );
        }
        assert_eq!(
            decode_one_byte_at_a_time(&bytes),
            whole,
            "case {case}: {bytes:02X?} one byte per call"
        );
    }
}

/// Exhaustive version over short strings of the interesting bytes: every string up to length 5
/// over a byte alphabet, split one byte per call and at every single cut.
#[test]
fn split_utf8_property_exhaustive_short_strings() {
    const ALPHABET: [u8; 10] = [0x41, 0x7F, 0x80, 0x99, 0xBF, 0xC2, 0xE2, 0xED, 0xF0, 0xFF];
    let mut bytes = Vec::new();
    for len in 0..=5usize {
        let total = ALPHABET.len().pow(len as u32);
        for mut code in 0..total {
            bytes.clear();
            for _ in 0..len {
                bytes.push(ALPHABET[code % ALPHABET.len()]);
                code /= ALPHABET.len();
            }
            let whole = decode_whole(&bytes);
            assert_eq!(decode_one_byte_at_a_time(&bytes), whole, "{bytes:02X?}");
            for cut in 0..=bytes.len() {
                assert_eq!(
                    decode_split(&bytes, &[cut]),
                    whole,
                    "{bytes:02X?} cut {cut}"
                );
            }
        }
    }
}

/// Whole-buffer output stays upstream's: spot checks of the invalid-input rules taken from the
/// oracle's behaviour (overlong, surrogate, BOM, bad continuation re-read, stray bytes).
#[test]
fn whole_buffer_invalid_input_matches_upstream_rules() {
    let cases: &[(&[u8], &[u32])] = &[
        (&[0xC0, 0x80, 0x41], &[0x41]),             // overlong 2-byte
        (&[0xC1, 0xBF, 0x41], &[0x41]),             // overlong 2-byte
        (&[0xE0, 0x80, 0x80, 0x41], &[0x41]),       // overlong 3-byte
        (&[0xED, 0xA0, 0x80, 0x41], &[0x41]),       // surrogate
        (&[0xEF, 0xBB, 0xBF, 0x41], &[0x41]),       // BOM dropped
        (&[0xF0, 0x80, 0x80, 0x80, 0x41], &[0x41]), // overlong 4-byte
        (&[0xF4, 0x90, 0x80, 0x80, 0x41], &[0x41]), // above U+10FFFF
        (&[0xE2, 0x41], &[0x41]),                   // bad continuation re-read
        (&[0xE2, 0x82, 0x41], &[0x41]),             // bad continuation after one good
        (&[0xF8, 0x80, 0xFF, 0x42], &[0x42]),       // stray bytes
        (&[0x80, 0xBF, 0x41], &[0x41]),             // stray continuation
        (&[0xC3, 0xC3, 0xA4], &[0xE4]),             // lead, then a fresh lead
    ];
    for (input, expected) in cases {
        let (out, interim) = decode_whole(input);
        assert_eq!(out, *expected, "{input:02X?}");
        assert!(interim.is_empty(), "{input:02X?}");
    }
}

/// Rust-only: `decode` clears the target, an empty input clears it too, and the count matches.
#[test]
fn decode_clears_target_and_returns_the_count() {
    let mut d = Utf8ToUtf32::new();
    let mut target = vec![1, 2, 3];
    assert_eq!(d.decode(b"ab", &mut target), 2);
    assert_eq!(target, [0x61, 0x62]);
    assert_eq!(d.decode(b"", &mut target), 0);
    assert!(target.is_empty());
}

/// Rust-only: every possible byte, alone and after a lead byte, never panics.
#[test]
fn decode_never_panics_on_any_single_byte_pair() {
    let mut target = Vec::new();
    for a in 0..=255u8 {
        for b in 0..=255u8 {
            let mut d = Utf8ToUtf32::new();
            d.decode(&[a], &mut target);
            d.decode(&[b], &mut target);
            d.decode(&[b, a, b, a, b], &mut target);
            assert!(d.interim().len() <= INTERIM_CAPACITY);
        }
    }
}
