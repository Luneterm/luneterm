// lunegit-oracle: writes Luneterm's Lunegit input/OSC goldens by calling Lunegit's own (prebuilt)
// encoders and scanners through reflection. No expected value in this file is typed by hand: every
// output column is whatever Lunegit's code returned. See README.md and
// crates/lune-vt-conformance/FORMAT.md ("Lunegit input goldens").

using System.Globalization;
using System.Reflection;
using System.Runtime.Loader;
using System.Security.Cryptography;
using System.Text;

namespace LunegitOracle;

internal delegate void FeedDelegate(ReadOnlySpan<byte> input);

internal static class Program
{
    private const string ToolVersion = "1";

    private static int Main(string[] args)
    {
        try
        {
            return Run(args);
        }
        catch (OracleException e)
        {
            Console.Error.WriteLine("lunegit-oracle: " + e.Message);
            return 2;
        }
    }

    private static int Run(string[] args)
    {
        string? outDir = null;
        string commit = string.Empty;
        for (int i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--out" when i + 1 < args.Length:
                    outDir = args[++i];
                    break;
                case "--commit" when i + 1 < args.Length:
                    commit = args[++i];
                    break;
                default:
                    throw new OracleException("usage: lunegit-oracle --out <dir> --commit <hex>");
            }
        }

        if (outDir is null || commit.Length == 0)
        {
            throw new OracleException("usage: lunegit-oracle --out <dir> --commit <hex>");
        }

        string? bin = Environment.GetEnvironmentVariable("LUNEGIT_BIN");
        if (string.IsNullOrEmpty(bin))
        {
            throw new OracleException(
                "LUNEGIT_BIN is not set. Point it at a directory holding Lunegit's built Lunegit.Core.dll and "
                + "Lunegit.Infrastructure.dll (for example <Lunegit>/src/Lunegit.Tests/bin/Debug/net10.0).");
        }

        var lunegit = LunegitTypes.Load(bin);
        Directory.CreateDirectory(outDir);
        var header = new List<(string, string)>
        {
            ("tool", "lunegit-oracle " + ToolVersion),
            ("lunegit-commit", commit),
            ("core-dll-sha256", lunegit.CoreSha256),
            ("infrastructure-dll-sha256", lunegit.InfrastructureSha256),
        };

        Keys.Write(Path.Combine(outDir, "keys.golden"), header, lunegit);
        Osc52.Write(Path.Combine(outDir, "osc52.golden"), header, lunegit);
        Scanners.WriteCwd(Path.Combine(outDir, "osc-cwd.golden"), header, lunegit);
        Scanners.WriteProgress(Path.Combine(outDir, "osc-progress.golden"), header, lunegit);
        Scanners.WriteCommand(Path.Combine(outDir, "osc-command.golden"), header, lunegit);
        return 0;
    }
}

internal sealed class OracleException(string message) : Exception(message);

// ---- reflection over Lunegit's DLLs ------------------------------------------------------------

internal sealed class LunegitTypes
{
    public required string CoreSha256 { get; init; }
    public required string InfrastructureSha256 { get; init; }
    public required Type KeyEvent { get; init; }
    public required Type Key { get; init; }
    public required Type Modifiers { get; init; }
    public required Type KeyboardModes { get; init; }
    public required Type KeyEncoder { get; init; }
    public required Type VtInputEncoder { get; init; }
    public required Type Osc52Parser { get; init; }
    public required Type CwdScanner { get; init; }
    public required Type ProgressScanner { get; init; }
    public required Type CommandScanner { get; init; }

    private const BindingFlags Any =
        BindingFlags.Static | BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic;

    public static LunegitTypes Load(string bin)
    {
        string corePath = Path.Combine(bin, "Lunegit.Core.dll");
        string infraPath = Path.Combine(bin, "Lunegit.Infrastructure.dll");
        foreach (string p in new[] { corePath, infraPath })
        {
            if (!File.Exists(p))
            {
                throw new OracleException(
                    $"{Path.GetFileName(p)} not found in LUNEGIT_BIN (build Lunegit, or point LUNEGIT_BIN at its test bin dir).");
            }
        }

        AssemblyLoadContext.Default.Resolving += (_, name) =>
        {
            string candidate = Path.Combine(bin, name.Name + ".dll");
            return File.Exists(candidate) ? AssemblyLoadContext.Default.LoadFromAssemblyPath(candidate) : null;
        };

        var core = Assembly.LoadFrom(corePath);
        var infra = Assembly.LoadFrom(infraPath);
        return new LunegitTypes
        {
            CoreSha256 = Sha256Hex(File.ReadAllBytes(corePath)),
            InfrastructureSha256 = Sha256Hex(File.ReadAllBytes(infraPath)),
            KeyEvent = Find(core, "Lunegit.Core.Terminal.TerminalKeyEvent"),
            Key = Find(core, "Lunegit.Core.Terminal.TerminalKey"),
            Modifiers = Find(core, "Lunegit.Core.Terminal.TerminalModifiers"),
            KeyboardModes = Find(infra, "Lunegit.Infrastructure.Terminal.KeyboardModes"),
            KeyEncoder = Find(infra, "Lunegit.Infrastructure.Terminal.TerminalKeyEncoder"),
            VtInputEncoder = Find(infra, "Lunegit.Infrastructure.Terminal.XtermVtInputEncoder"),
            Osc52Parser = Find(infra, "Lunegit.Infrastructure.Terminal.Osc52Parser"),
            CwdScanner = Find(infra, "Lunegit.Infrastructure.Terminal.OscCwdScanner"),
            ProgressScanner = Find(infra, "Lunegit.Infrastructure.Terminal.Osc94ProgressScanner"),
            CommandScanner = Find(infra, "Lunegit.Infrastructure.Terminal.TerminalCommandScanner"),
        };
    }

    private static Type Find(Assembly asm, string fullName) =>
        asm.GetType(fullName, throwOnError: false)
        ?? throw new OracleException($"type {fullName} not found in {asm.GetName().Name} (renamed internals?)");

    public static string Sha256Hex(byte[] data) => Convert.ToHexString(SHA256.HashData(data)).ToLowerInvariant();

    public static MethodInfo Method(Type t, string name) =>
        t.GetMethod(name, Any) ?? throw new OracleException($"method {t.Name}.{name} not found");

    public static PropertyInfo Property(Type t, string name) =>
        t.GetProperty(name, Any) ?? throw new OracleException($"property {t.Name}.{name} not found");

    /// <summary>Invokes <paramref name="m"/>, rethrowing whatever Lunegit's code threw.</summary>
    public static object? Call(MethodInfo m, object? target, params object?[] args)
    {
        try
        {
            return m.Invoke(target, args);
        }
        catch (TargetInvocationException e) when (e.InnerException is not null)
        {
            throw new OracleException($"{m.DeclaringType?.Name}.{m.Name} threw {e.InnerException.GetType().Name}");
        }
    }

    public object NewKeyEvent(int key, int mods, int cp, int shifted, int baseLayout, bool keypad, bool altIsMeta) =>
        Activator.CreateInstance(
            KeyEvent,
            Enum.ToObject(Key, key),
            Enum.ToObject(Modifiers, mods),
            cp,
            shifted,
            baseLayout,
            keypad,
            altIsMeta) ?? throw new OracleException("TerminalKeyEvent construction failed");
}

// ---- the golden writer and the byte-spec mini language -----------------------------------------

/// <summary>
/// Byte specs keep large inputs out of the files: parts joined by '+', each part lowercase hex
/// ("1b5d") or hex repeated ("61x4096"); "-" is empty. The tool builds a spec and then expands it
/// with the same code the Rust reader mirrors, so the spec is what was fed to Lunegit.
/// </summary>
internal static class Spec
{
    public const string Empty = "-";

    public static string Lit(string text) => Raw(Encoding.UTF8.GetBytes(text));

    public static string Raw(params byte[] bytes) =>
        bytes.Length == 0 ? Empty : Convert.ToHexString(bytes).ToLowerInvariant();

    public static string Rep(byte b, int count) => count == 0 ? Empty : $"{b:x2}x{count}";

    public static string Cat(params string[] parts)
    {
        var kept = parts.Where(p => p != Empty).ToArray();
        return kept.Length == 0 ? Empty : string.Join("+", kept);
    }

    public static byte[] Expand(string spec)
    {
        if (spec == Empty)
        {
            return [];
        }

        var bytes = new List<byte>();
        foreach (string part in spec.Split('+'))
        {
            int x = part.IndexOf('x');
            string hex = x < 0 ? part : part[..x];
            int count = x < 0 ? 1 : int.Parse(part[(x + 1)..], CultureInfo.InvariantCulture);
            byte[] unit = Convert.FromHexString(hex);
            for (int i = 0; i < count; i++)
            {
                bytes.AddRange(unit);
            }
        }

        return [.. bytes];
    }

    public static string Utf8Hex(string text) => Raw(Encoding.UTF8.GetBytes(text));

    /// <summary>The golden string escaping (FORMAT.md): printable ASCII verbatim except " and \, else \u{X}.</summary>
    public static string Escape(string text)
    {
        var sb = new StringBuilder("\"");
        foreach (var rune in text.EnumerateRunes())
        {
            int v = rune.Value;
            if (v is >= 0x20 and <= 0x7E && v != '"' && v != '\\')
            {
                sb.Append((char)v);
            }
            else
            {
                sb.Append("\\u{").Append(v.ToString("X", CultureInfo.InvariantCulture)).Append('}');
            }
        }

        return sb.Append('"').ToString();
    }
}

internal sealed class GoldenFile(string kind, List<(string, string)> header)
{
    private readonly StringBuilder _sb = new();
    private int _columns;
    private string _section = string.Empty;

    public void Section(string name, string[] inputs, string[] outputs)
    {
        if (_section.Length != 0)
        {
            _sb.Append("end\n");
        }

        _section = name;
        _columns = inputs.Length + outputs.Length;
        _sb.Append("section ").Append(name).Append('\n');
        _sb.Append("inputs ").Append(string.Join(' ', inputs)).Append('\n');
        _sb.Append("outputs ").Append(string.Join(' ', outputs)).Append('\n');
    }

    public void Row(params string[] fields)
    {
        if (fields.Length != _columns)
        {
            throw new OracleException($"section {_section}: row has {fields.Length} fields, expected {_columns}");
        }

        foreach (string f in fields)
        {
            if (f.Length == 0 || f.Length > MaxChars || f.Contains('\t') || f.Contains('\n') || f.Contains('\r'))
            {
                throw new OracleException($"section {_section}: bad field (length {f.Length})");
            }
        }

        _sb.Append(string.Join('\t', fields)).Append('\n');
    }

    private const int MaxChars = 256;

    public void Write(string path)
    {
        if (_section.Length != 0)
        {
            _sb.Append("end\n");
            _section = string.Empty;
        }

        var head = new StringBuilder();
        head.Append("lune-lunegit-golden 1\n");
        foreach (var (k, v) in header)
        {
            head.Append("header ").Append(k).Append(' ').Append(v).Append('\n');
        }

        head.Append("kind ").Append(kind).Append('\n');
        File.WriteAllText(path, head.ToString() + _sb, new UTF8Encoding(false));
    }
}

// ---- keys ---------------------------------------------------------------------------------------

internal static class Keys
{
    // KeyboardModes.Pack layout: bits 0-7 kitty flags, bits 8-9 modifyOtherKeys, bit 16 DECCKM.
    private static int Pack(int kitty, int mok, bool decckm) => kitty | (mok << 8) | (decckm ? 1 << 16 : 0);

    private static readonly int Legacy = Pack(0, 0, false);
    private static readonly int Decckm = Pack(0, 0, true);
    private static readonly int Kitty1 = Pack(1, 0, false);
    private static readonly int Kitty5 = Pack(5, 0, false);
    private static readonly int Kitty4 = Pack(4, 0, false);
    private static readonly int Mok1 = Pack(0, 1, false);
    private static readonly int Mok2 = Pack(0, 2, false);
    private static readonly int Kitty1Mok2 = Pack(1, 2, false);

    private static readonly int[] AllModes = [Legacy, Decckm, Kitty1, Kitty5, Kitty4, Mok1, Mok2, Kitty1Mok2];
    private static readonly int[] CoreVariantModes = [Legacy, Kitty1, Kitty5, Mok2];
    private static readonly int[] BroadModes = [Legacy, Kitty5, Mok2];

    // Keys where a numeric-keypad variant means something to the encoder.
    private static readonly HashSet<string> KeypadKeys =
        ["Enter", "Up", "Down", "Right", "Left", "Home", "End", "PageUp", "PageDown", "Insert", "Delete"];

    private static string Hex(string text) => Spec.Utf8Hex(text);

    public static void Write(string path, List<(string, string)> header, LunegitTypes t)
    {
        var encode = LunegitTypes.Method(t.KeyEncoder, "Encode");
        var unpack = LunegitTypes.Method(t.KeyboardModes, "Unpack");
        var legacyEncoder = Activator.CreateInstance(t.VtInputEncoder, nonPublic: true)
            ?? throw new OracleException("XtermVtInputEncoder construction failed");
        var genKey = LunegitTypes.Method(t.VtInputEncoder, "GenerateKeyInput");
        var genChar = LunegitTypes.Method(t.VtInputEncoder, "GenerateCharInput");

        string Encode(object ev, int modes) =>
            (string)LunegitTypes.Call(encode, null, ev, LunegitTypes.Call(unpack, null, modes))!;

        var file = new GoldenFile("keys", header);
        string[] keyNames = Enum.GetNames(t.Key);
        int characterKey = Array.IndexOf(keyNames, "Character");

        // 1. Functional keys: every key x 8 modifier sets x AltIsMeta x 8 modes (x keypad where meaningful).
        file.Section("encode-key", ["key", "keypad", "mods", "alt", "modes"], ["out"]);
        for (int k = 0; k < keyNames.Length; k++)
        {
            if (k == characterKey)
            {
                continue;
            }

            foreach (bool keypad in KeypadKeys.Contains(keyNames[k]) ? new[] { false, true } : new[] { false })
            {
                foreach (int mods in Enumerable.Range(0, 8))
                {
                    foreach (bool alt in new[] { true, false })
                    {
                        // The keypad flag changes bytes only under the kitty protocol; the other modes
                        // get one representative each.
                        foreach (int modes in keypad ? CoreVariantModes : AllModes)
                        {
                            var ev = t.NewKeyEvent(k, mods, 0, 0, 0, keypad, alt);
                            file.Row(keyNames[k], keypad ? "1" : "0", mods.ToString(), alt ? "1" : "0",
                                modes.ToString(), Hex(Encode(ev, modes)));
                        }
                    }
                }
            }
        }

        // 2. Character keys.
        file.Section("encode-char", ["cp", "shifted", "base", "keypad", "mods", "alt", "modes"], ["out"]);
        void CharRows(int cp, int shifted, int baseLayout, bool keypad, int[] altValues, int[] modeSet)
        {
            foreach (int mods in Enumerable.Range(0, 8))
            {
                foreach (int alt in altValues)
                {
                    foreach (int modes in modeSet)
                    {
                        var ev = t.NewKeyEvent(characterKey, mods, cp, shifted, baseLayout, keypad, alt == 1);
                        file.Row(cp.ToString("x"), shifted.ToString("x"), baseLayout.ToString("x"),
                            keypad ? "1" : "0", mods.ToString(), alt.ToString(), modes.ToString(),
                            Hex(Encode(ev, modes)));
                    }
                }
            }
        }

        var coreCorpus = CoreCorpus();
        var variantCorpus = VariantCorpus();
        var coreSet = new HashSet<int>(coreCorpus);
        int[] bothAlt = [1, 0];
        int[] altOnly = [1];

        foreach (int cp in coreCorpus)
        {
            CharRows(cp, 0, 0, false, bothAlt, AllModes);
        }

        foreach (int cp in variantCorpus)
        {
            int shifted = ShiftedOf(cp);
            int baseLayout = BaseLayoutOf(cp);
            CharRows(cp, shifted, 0, false, altOnly, CoreVariantModes);
            CharRows(cp, 0, baseLayout, false, altOnly, CoreVariantModes);
            CharRows(cp, shifted, baseLayout, false, altOnly, CoreVariantModes);
            CharRows(cp, 0, cp, false, altOnly, CoreVariantModes);
        }

        for (int cp = 0x20; cp <= 0x7E; cp++)
        {
            if (!coreSet.Contains(cp))
            {
                CharRows(cp, 0, 0, false, altOnly, BroadModes);
            }
        }

        foreach (int cp in "0123456789./*-+=,".Select(c => (int)c))
        {
            CharRows(cp, 0, 0, true, altOnly, BroadModes);
        }

        // Two codepoints that are not scalar values: Lunegit sends nothing in any mode.
        foreach (int cp in new[] { 0xD800, 0x110000, -1 })
        {
            CharRows(cp, 0, 0, false, altOnly, [Legacy, Kitty1, Mok2]);
        }

        // 3. The legacy generator (the parity surface).
        file.Section("generate-key-input", ["key", "mods", "decckm"], ["out"]);
        for (int k = 0; k < keyNames.Length; k++)
        {
            foreach (int mods in Enumerable.Range(0, 8))
            {
                foreach (bool decckm in new[] { false, true })
                {
                    string s = (string)LunegitTypes.Call(genKey, legacyEncoder,
                        Enum.ToObject(t.Key, k), Enum.ToObject(t.Modifiers, mods), decckm)!;
                    file.Row(keyNames[k], mods.ToString(), decckm ? "1" : "0", Hex(s));
                }
            }
        }

        file.Section("generate-char-input", ["char", "mods"], ["out"]);
        var chars = new List<char>();
        for (char c = (char)0x01; c <= 0x7F; c++)
        {
            chars.Add(c);
        }

        chars.AddRange("\u00e4\u00e9\u20ac\u00df\u0444\u4e2d");
        foreach (char c in chars)
        {
            foreach (int mods in Enumerable.Range(0, 8))
            {
                string s = (string)LunegitTypes.Call(genChar, legacyEncoder, c, Enum.ToObject(t.Modifiers, mods))!;
                file.Row(((int)c).ToString("x"), mods.ToString(), Hex(s));
            }
        }

        file.Write(path);
    }

    // Codepoints for the full grid: the ones the legacy Ctrl table, the kitty/MOK letter and digit
    // handling, and non-ASCII text all branch on.
    private static List<int> CoreCorpus()
    {
        var list = new List<int>();
        list.AddRange("azA05 -[;".Select(c => (int)c));
        list.AddRange([0xE4, 0xE9, 0x20AC, 0x4E2D, 0x1F600]);
        return list;
    }

    private static List<int> VariantCorpus()
    {
        var list = new List<int>();
        list.AddRange("azA5;-[".Select(c => (int)c));
        list.AddRange([0xE4, 0x444, 0x4E2D]);
        return list;
    }

    // A shifted codepoint distinct from the base: US layout for ASCII, the capital for letters.
    private static int ShiftedOf(int cp)
    {
        const string from = "`1234567890-=[]\\;',./";
        const string to = "~!@#$%^&*()_+{}|:\"<>?";
        if (cp is >= 'a' and <= 'z')
        {
            return cp - 0x20;
        }

        int i = cp < 0x80 ? from.IndexOf((char)cp) : -1;
        if (i >= 0)
        {
            return to[i];
        }

        return cp switch
        {
            0xE4 => 0xC4,
            0xE9 => 0xC9,
            0x444 => 0x424,
            _ => cp == 'A' ? 'A' : cp + 1,
        };
    }

    // A base-layout codepoint different from the key's own.
    private static int BaseLayoutOf(int cp) => cp switch
    {
        0xE4 => 'a',
        0xE9 => 'e',
        0x444 => 'a',
        'q' => 'w',
        _ => 'q',
    };
}

// ---- OSC 52 -------------------------------------------------------------------------------------

internal static class Osc52
{
    private const int MaxBase64Chars = 4 * ((1024 * 1024 + 2) / 3);

    public static void Write(string path, List<(string, string)> header, LunegitTypes t)
    {
        var parse = LunegitTypes.Method(t.Osc52Parser, "Parse");
        var file = new GoldenFile("osc52", header);
        file.Section("parse", ["name", "payload", "text"], ["outcome", "length", "text-bytes", "text-hex", "text-sha256"]);

        string B64(params byte[] b) => Convert.ToBase64String(b);
        byte[] Utf8(string s) => Encoding.UTF8.GetBytes(s);
        var cases = new List<(string Name, string? Payload)>();
        void Add(string name, string? payload) => cases.Add((name, payload));

        Add("null", null);
        Add("empty", "");
        Add("no_separator", "c");
        Add("no_separator_b64", B64(Utf8("hello")));
        Add("sep_only", ";");
        Add("sel_sep", "c;");
        Add("query", "c;?");
        Add("query_empty_sel", ";?");
        Add("query_other_sel", "p;?");
        Add("query_digit_sel", "0;?");
        Add("query_x", "c;?x");
        Add("query_double", "c;??");
        Add("query_space", "c; ?");
        Add("query_trailing_space", "c;? ");
        Add("sel_c_hello", "c;" + B64(Utf8("hello")));
        Add("sel_p_hello", "p;" + B64(Utf8("hello")));
        Add("sel_s_hello", "s;" + B64(Utf8("hello")));
        Add("sel_digit_hello", "0;" + B64(Utf8("hello")));
        Add("sel_multi_hello", "cp;" + B64(Utf8("hello")));
        Add("sel_empty_hello", ";" + B64(Utf8("hello")));
        Add("sel_garbage_hello", "zzz;" + B64(Utf8("hello")));
        Add("sel_with_semi", "c;;" + B64(Utf8("hello")));
        Add("second_semi_in_body", "c;" + B64(Utf8("hello")) + ";x");
        Add("utf8_text", "c;" + B64(Utf8("h\u00e9llo w\u00f6rld \u20ac \u4e2d\u6587 \U0001F600")));
        Add("ascii_controls", "c;" + B64(Utf8("a\tb\nc\r\nd\u001be\u0000f")));
        Add("lone_nul", "c;" + B64(0));
        Add("invalid_utf8_ff", "c;" + B64(0xFF, 0xFE, 0x41));
        Add("invalid_utf8_truncated", "c;" + B64(0x41, 0xC3));
        Add("invalid_utf8_overlong", "c;" + B64(0xC0, 0xAF));
        Add("invalid_utf8_surrogate", "c;" + B64(0xED, 0xA0, 0x80));
        Add("utf8_bom", "c;" + B64(0xEF, 0xBB, 0xBF, 0x41));
        Add("len1", "c;" + B64(0x61));
        Add("len2", "c;" + B64(0x61, 0x62));
        Add("len3", "c;" + B64(0x61, 0x62, 0x63));
        Add("len4", "c;" + B64(0x61, 0x62, 0x63, 0x64));
        Add("no_pad_len2", "c;YQ");
        Add("no_pad_len3", "c;YWI");
        Add("one_char", "c;Y");
        Add("five_chars", "c;YWJjZ");
        Add("six_chars", "c;YWJjZA");
        Add("seven_chars", "c;YWJjZGU");
        foreach (var (wsName, ws) in new[]
        {
            ("space", " "), ("tab", "\t"), ("cr", "\r"), ("lf", "\n"), ("vt", "\u000b"), ("ff", "\u000c"),
            ("crlf", "\r\n"), ("nbsp", "\u00a0"), ("nel", "\u0085"), ("ls", "\u2028"), ("nul", "\u0000"),
            ("zwsp", "\u200b"), ("fs", "\u001c"),
        })
        {
            Add("ws_mid_" + wsName, "c;aGVs" + ws + "bG8=");
            Add("ws_lead_" + wsName, "c;" + ws + "aGVsbG8=");
            Add("ws_trail_" + wsName, "c;aGVsbG8=" + ws);
            Add("ws_in_quad_" + wsName, "c;aG" + ws + "VsbG8=");
            Add("ws_before_pad_" + wsName, "c;aGVsbG" + ws + "8=");
            Add("ws_in_pad_" + wsName, "c;YQ=" + ws + "=");
            Add("ws_only_" + wsName, "c;" + ws + ws + ws);
        }

        Add("ws_wrapped_lines", "c;aGVsbG8gd29y\r\nbGQ=\r\n");
        Add("pad_only4", "c;====");
        Add("pad_only1", "c;=");
        Add("pad_only2", "c;==");
        Add("pad_only3", "c;===");
        Add("pad_1_of_4", "c;YWJj=");
        Add("pad_ok_1", "c;YWI=");
        Add("pad_ok_2", "c;YQ==");
        Add("pad_three", "c;YQ===");
        Add("pad_short", "c;YQ=");
        Add("pad_then_data", "c;YQ==YQ==");
        Add("pad_mid_quad", "c;Y=Q=");
        Add("pad_lead_quad", "c;=YQ=");
        Add("pad_after_one", "c;Y===");
        Add("pad_four_then_data", "c;====YWJj");
        Add("data_then_four_pad", "c;YWJj====");
        Add("nonzero_pad_bits_1", "c;YR==");
        Add("nonzero_pad_bits_2", "c;YWJ=");
        Add("nonzero_pad_bits_3", "c;Zh==");
        Add("zero_pad_bits", "c;Zg==");
        Add("bad_char_bang", "c;aGVs!G8=");
        Add("bad_char_dot", "c;aGVs.G8=");
        Add("bad_char_urlsafe", "c;aGVs-_8=");
        Add("bad_char_e_acute", "c;aGVs\u00e9G8=");
        Add("bad_char_fullwidth", "c;\uff21GVsbG8=");
        Add("bad_char_nul", "c;aGVs\u0000bG8=");
        Add("bad_char_comma", "c;aGVs,bG8=");
        Add("bad_char_quote", "c;aGVs\"bG8=");
        Add("bad_plus_slash_ok", "c;+/+/");
        Add("bad_hash", "c;#");
        Add("empty_after_ws", "c;\t\r\n ");

        var lines = new List<(string Name, string Spec, string Text, string? Payload)>();
        foreach (var (name, payload) in cases)
        {
            string spec = payload is null ? "null" : Spec.Lit(payload);
            if (spec.Length > 240)
            {
                throw new OracleException($"corpus case {name} needs a generator directive");
            }

            lines.Add((name, spec, payload is null ? "null" : Spec.Escape(payload), payload));
        }

        // Generated large inputs: stored as repeat directives, never inline.
        string P(string prefix, params string[] rest) => Spec.Cat(Spec.Lit(prefix), Spec.Cat(rest));
        string A(int n) => Spec.Rep((byte)'A', n);
        void AddSpec(string name, string spec) => lines.Add((name, spec, "-", null));

        AddSpec("big_exact_max_chars_a", P("c;", A(MaxBase64Chars)));
        AddSpec("big_max_chars_plus1", P("c;", A(MaxBase64Chars + 1)));
        AddSpec("big_max_chars_minus1", P("c;", A(MaxBase64Chars - 1)));
        AddSpec("big_decoded_exactly_1mib", P("c;", A(MaxBase64Chars - 4), Spec.Lit("AA==")));
        AddSpec("big_decoded_1mib_plus1", P("c;", A(MaxBase64Chars - 4), Spec.Lit("AAA=")));
        AddSpec("big_decoded_1mib_plus2", P("c;", A(MaxBase64Chars)));
        AddSpec("big_decoded_1mib_minus1", P("c;", A(MaxBase64Chars - 4)));
        AddSpec("long_valid_repeat", P("c;", Spec.Lit("YWJjZGVm") + "x1000"));
        AddSpec("big_100k_chars", P("c;", A(100000)));
        AddSpec("big_2x_cap", P("c;", A(2 * MaxBase64Chars)));
        AddSpec("big_bad_char_at_end", P("c;", A(MaxBase64Chars - 1), Spec.Lit("!")));
        AddSpec("big_ws_padded", P("c;", Spec.Rep((byte)' ', 5000), Spec.Lit("YQ==")));
        AddSpec("big_ws_only", P("c;", Spec.Rep((byte)'\n', 5000)));
        AddSpec("big_ws_pushes_past_cap", P("c;", A(MaxBase64Chars), Spec.Rep((byte)'\n', 10)));
        AddSpec("big_selection_long", P("", Spec.Rep((byte)'c', 5000), Spec.Lit(";YQ==")));
        AddSpec("big_query_after_long_selection", P("", Spec.Rep((byte)'c', 5000), Spec.Lit(";?")));

        foreach (var (name, spec, text, _) in lines)
        {
            string? payload = spec == "null" ? null : Encoding.UTF8.GetString(Spec.Expand(spec));
            object? result = LunegitTypes.Call(parse, null, payload);
            var type = result!.GetType();
            string outcome = LunegitTypes.Property(type, "Outcome").GetValue(result)!.ToString()!;
            string? textOut = (string?)LunegitTypes.Property(type, "Text").GetValue(result);
            int length = (int)LunegitTypes.Property(type, "PayloadLength").GetValue(result)!;
            byte[] textBytes = textOut is null ? [] : Encoding.UTF8.GetBytes(textOut);
            string textHex = textOut is null || textBytes.Length > 96 ? "-" : Spec.Raw(textBytes);
            string sha = textOut is null ? "-" : LunegitTypes.Sha256Hex(textBytes);
            file.Row(name, spec, text, outcome, length.ToString(), textOut is null ? "-" : textBytes.Length.ToString(),
                textHex, sha);
        }

        file.Write(path);
    }
}

// ---- the three byte scanners --------------------------------------------------------------------

internal static class Scanners
{
    private static string Esc => "\u001b";

    // One OSC, framed as Lunegit's scanners (and every real emitter) frame it.
    private static string Osc(string body, bool st = false) =>
        Spec.Cat(Spec.Raw(0x1B, 0x5D), Spec.Lit(body), st ? Spec.Raw(0x1B, 0x5C) : Spec.Raw(0x07));

    private static string OscPad(string prefix, int totalBody, bool st = false) =>
        Spec.Cat(Spec.Raw(0x1B, 0x5D), Spec.Lit(prefix), Spec.Rep((byte)'a', totalBody - prefix.Length),
            st ? Spec.Raw(0x1B, 0x5C) : Spec.Raw(0x07));

    private static string Csi(string text) => Spec.Lit(Esc + "[" + text);

    private sealed record Case(string Name, string[] Feeds);

    private static List<Case> One(IEnumerable<(string Name, string Body)> bodies, bool alsoSt)
    {
        var list = new List<Case>();
        foreach (var (name, body) in bodies)
        {
            list.Add(new Case(name, [Osc(body)]));
            if (alsoSt)
            {
                list.Add(new Case(name + "_st", [Osc(body, st: true)]));
            }
        }

        return list;
    }

    private static void Run(string path, string kind, List<(string, string)> header, string[] outputs,
        List<Case> cases, Func<(FeedDelegate Feed, Func<string[]> State)> make)
    {
        var file = new GoldenFile(kind, header);
        file.Section("feed", ["case", "step", "bytes"], outputs);
        var seen = new HashSet<string>();
        foreach (var c in cases)
        {
            if (!seen.Add(c.Name))
            {
                throw new OracleException("duplicate case " + c.Name);
            }

            var (feed, state) = make();
            for (int i = 0; i < c.Feeds.Length; i++)
            {
                feed(Spec.Expand(c.Feeds[i]));
                file.Row([c.Name, i.ToString(), c.Feeds[i], .. state()]);
            }
        }

        file.Write(path);
    }

    private static (FeedDelegate, object) Create(Type type)
    {
        object scanner = Activator.CreateInstance(type, nonPublic: true)
            ?? throw new OracleException(type.Name + " construction failed");
        var feed = (FeedDelegate)LunegitTypes.Method(type, "Feed").CreateDelegate(typeof(FeedDelegate), scanner);
        return (feed, scanner);
    }

    // ---- cwd (OSC 7, OSC 9;9) ----

    public static void WriteCwd(string path, List<(string, string)> header, LunegitTypes t)
    {
        var cases = new List<Case>();
        cases.AddRange(One(
        [
            ("osc7_plain", "7;file:///home/user"),
            ("osc7_host", "7;file://myhost/home/user"),
            ("osc7_localhost", "7;file://localhost/tmp/x"),
            ("osc7_escaped_space", "7;file:///home/a%20b"),
            ("osc7_escaped_utf8", "7;file:///home/%C3%A4%E4%B8%AD"),
            ("osc7_escaped_emoji", "7;file:///home/%F0%9F%98%80"),
            ("osc7_truncated_escape", "7;file:///%E0%A4%A"),
            ("osc7_dos_path", "7;file:///C:/Users/x"),
            ("osc7_dos_path_lower", "7;file:///c:/"),
            ("osc7_dos_path_host", "7;file://host/C:/Users/x"),
            ("osc7_root", "7;file:///"),
            ("osc7_no_path", "7;file://"),
            ("osc7_scheme_only", "7;file:"),
            ("osc7_host_only", "7;file://host"),
            ("osc7_host_slash", "7;file://host/"),
            ("osc7_empty", "7;"),
            ("osc7_blank", "7;   "),
            ("osc7_plain_path", "7;/plain/path"),
            ("osc7_http", "7;http://example.com/x"),
            ("osc7_upper_scheme", "7;FILE:///Upper/Case"),
            ("osc7_escaped_slash", "7;file:///a%2Fb"),
            ("osc7_bad_escape", "7;file:///a%zz"),
            ("osc7_trailing_percent", "7;file:///a%"),
            ("osc7_query_fragment", "7;file:///a?q=1#frag"),
            ("osc7_trailing_slash", "7;file:///tmp/"),
            ("osc7_raw_utf8", "7;file:///home/\u00e4"),
            ("osc7_raw_cjk", "7;file:///home/\u4e2d\u6587"),
            ("osc7_trim", "7;  file:///trim/me  "),
            ("osc7_raw_space", "7;file:///a b"),
            ("osc7_nul_escape", "7;file:///%00"),
            ("osc7_four_slashes", "7;file:////double"),
            ("osc7_dot_dot", "7;file:///a/../b"),
            ("osc7_dot", "7;file:///a/./b"),
            ("osc7_userinfo", "7;file://user@host/x"),
            ("osc7_port", "7;file://host:80/x"),
            ("osc7_backslash", "7;file:///a\\b"),
            ("osc7_semicolon", "7;file:///home/u;semi"),
            ("osc7_double_prefix", "7;7;file:///x"),
            ("osc9_9_windows", "9;9;C:\\Users\\x"),
            ("osc9_9_unix", "9;9;/home/u"),
            ("osc9_9_file_uri", "9;9;file:///C:/x"),
            ("osc9_9_file_uri_host", "9;9;file://h/p"),
            ("osc9_9_empty", "9;9;"),
            ("osc9_9_trim", "9;9;  /x  "),
            ("osc9_9_tilde", "9;9;~"),
            ("osc9_9_relative", "9;9;relative/dir"),
            ("osc9_9_utf8", "9;9;\u4e2d\u6587/\u00e4"),
            ("osc9_9_file_scheme_upper", "9;9;FILE:/x"),
            ("osc9_9_file_scheme_only", "9;9;file:"),
            ("osc9_9_no_trailing_semi", "9;9"),
            ("osc9_9_double_semi", "9;9;;"),
            ("osc9_9_semicolon_in_path", "9;9;/a;b"),
            ("osc9_8", "9;8;/x"),
            ("osc9_4_progress", "9;4;1;50"),
            ("osc9_empty_sub", "9;;9;/x"),
            ("osc0_title", "0;title"),
            ("osc2_title", "2;title"),
            ("osc1_7", "1;7;file:///x"),
            ("osc77", "77;file:///x"),
            ("osc7_no_semicolon", "7file:///x"),
        ], alsoSt: false));
        cases.Add(new Case("osc7_plain_st_variant", [Osc("7;file:///st/term", st: true)]));
        cases.Add(new Case("osc9_9_st_variant", [Osc("9;9;/st/term", st: true)]));
        cases.Add(new Case("body_exactly_4096", [OscPad("7;file:///", 4096)]));
        cases.Add(new Case("body_4097", [OscPad("7;file:///", 4097)]));
        cases.Add(new Case("body_5000", [OscPad("7;file:///", 5000)]));
        cases.Add(new Case("body_4096_st", [OscPad("7;file:///", 4096, st: true)]));
        cases.Add(new Case("body_4097_st", [OscPad("7;file:///", 4097, st: true)]));
        cases.Add(new Case("osc9_9_body_exactly_4096", [OscPad("9;9;/", 4096)]));
        cases.Add(new Case("osc9_9_body_4097", [OscPad("9;9;/", 4097)]));
        cases.Add(new Case("overflow_then_valid", [OscPad("7;file:///", 4097), Osc("7;file:///after")]));
        cases.Add(new Case("valid_then_overflow", [Osc("7;file:///before"), OscPad("7;file:///", 4097)]));
        cases.Add(new Case("two_updates", [Osc("7;file:///a"), Osc("7;file:///b")]));
        cases.Add(new Case("osc7_then_osc9_9", [Osc("7;file:///a"), Osc("9;9;/b")]));
        cases.Add(new Case("invalid_keeps_latest", [Osc("7;file:///keep"), Osc("7;"), Osc("9;9;"), Osc("0;t")]));
        cases.Add(new Case("both_in_one_feed", [Spec.Cat(Osc("7;file:///a"), Osc("9;9;/b"))]));
        cases.Add(new Case("garbage_before", [Spec.Cat(Csi("1mhello"), Osc("7;file:///g"))]));
        cases.Add(new Case("split_bytes", [Spec.Raw(0x1B), Spec.Lit("]7;file:///sp"), Spec.Lit("lit/x"), Spec.Raw(0x07)]));
        cases.Add(new Case("split_st", [Spec.Cat(Spec.Raw(0x1B, 0x5D), Spec.Lit("7;file:///s")), Spec.Raw(0x1B), Spec.Raw(0x5C)]));
        cases.Add(new Case("split_utf8", [Spec.Cat(Spec.Raw(0x1B, 0x5D), Spec.Lit("7;file:///\u00e4").Replace("c3a4", "c3")), Spec.Raw(0xA4), Spec.Raw(0x07)]));
        cases.Add(new Case("one_byte_each", Spec.Expand(Osc("7;file:///one")).Select(b => Spec.Raw(b)).ToArray()));

        // A long cwd is recorded by length and SHA-256 only (no field may pass 256 characters).
        Run(path, "osc-cwd", header, ["cwd", "cwd-hex", "cwd-bytes", "cwd-sha256"], cases, () =>
        {
            var (feed, scanner) = Create(t.CwdScanner);
            var latest = LunegitTypes.Property(t.CwdScanner, "Latest");
            return (feed, () =>
            {
                string? v = (string?)latest.GetValue(scanner);
                if (v is null)
                {
                    return ["-", "-", "-", "-"];
                }

                byte[] bytes = Encoding.UTF8.GetBytes(v);
                bool small = bytes.Length <= 100;
                return [small ? Spec.Escape(v) : "-", small ? Spec.Raw(bytes) : "-",
                    bytes.Length.ToString(CultureInfo.InvariantCulture), LunegitTypes.Sha256Hex(bytes)];
            });
        });
    }

    // ---- progress (OSC 9;4) ----

    public static void WriteProgress(string path, List<(string, string)> header, LunegitTypes t)
    {
        var cases = new List<Case>();
        cases.AddRange(One(
        [
            ("state0", "9;4;0"),
            ("state1_50", "9;4;1;50"),
            ("state2_100", "9;4;2;100"),
            ("state3", "9;4;3"),
            ("state4_25", "9;4;4;25"),
            ("state5_unknown", "9;4;5;10"),
            ("state6_unknown", "9;4;6"),
            ("state1_empty_pct", "9;4;1;"),
            ("state1_alpha_pct", "9;4;1;abc"),
            ("state1_pct_150", "9;4;1;150"),
            ("state1_pct_huge", "9;4;1;99999999999"),
            ("state1_pct_max_int", "9;4;1;2147483647"),
            ("state1_pct_negative", "9;4;1;-5"),
            ("state1_pct_plus", "9;4;1;+5"),
            ("state1_pct_space", "9;4;1;5 0"),
            ("state1_pct_arabic_digit", "9;4;1;\u0663"),
            ("state1_pct_0", "9;4;1;0"),
            ("state1_pct_100", "9;4;1;100"),
            ("state1_pct_101", "9;4;1;101"),
            ("state3_pct_50", "9;4;3;50"),
            ("state0_pct_77", "9;4;0;77"),
            ("state_leading_zero", "9;4;01;050"),
            ("state_empty_pct_token", "9;4;;5"),
            ("state_huge", "9;4;99999999999;5"),
            ("state_alpha", "9;4;x;5"),
            ("state1_extra_field", "9;4;1;50;extra"),
            ("state1_trailing_semi", "9;4;1;50;"),
            ("state1_double_semi", "9;4;1;;"),
            ("no_state", "9;4;"),
            ("no_state_no_semi", "9;4"),
            ("sub5", "9;5;1;50"),
            ("sub9_cwd", "9;9;/x"),
            ("leading_semi", ";4;1;5"),
            ("osc94", "94;1;5"),
            ("title", "0;title"),
            ("sub4_no_percent_sep", "9;4;1;5;5;5"),
        ], alsoSt: true));
        cases.Add(new Case("body_exactly_64", [OscPad("9;4;1;50;", 64)]));
        cases.Add(new Case("body_65", [OscPad("9;4;1;50;", 65)]));
        cases.Add(new Case("body_64_st", [OscPad("9;4;1;50;", 64, st: true)]));
        cases.Add(new Case("body_65_st", [OscPad("9;4;1;50;", 65, st: true)]));
        cases.Add(new Case("body_200", [OscPad("9;4;1;50;", 200)]));
        cases.Add(new Case("overflow_then_valid", [OscPad("9;4;1;50;", 65), Osc("9;4;2;7")]));
        cases.Add(new Case("set_then_clear", [Osc("9;4;1;50"), Osc("9;4;0")]));
        cases.Add(new Case("set_then_invalid", [Osc("9;4;1;50"), Osc("9;4;x;1"), Osc("9;4"), Osc("0;t")]));
        cases.Add(new Case("set_then_unknown_state", [Osc("9;4;1;50"), Osc("9;4;9;60")]));
        cases.Add(new Case("progress_run", [Osc("9;4;3"), Osc("9;4;1;10"), Osc("9;4;1;60"), Osc("9;4;2;60"), Osc("9;4;0")]));
        cases.Add(new Case("both_in_one_feed", [Spec.Cat(Osc("9;4;1;20"), Osc("9;4;4;80"))]));
        cases.Add(new Case("garbage_before", [Spec.Cat(Csi("1mhello"), Osc("9;4;1;33"))]));
        cases.Add(new Case("split_bytes", [Spec.Raw(0x1B), Spec.Lit("]9;4;"), Spec.Lit("1;4"), Spec.Lit("2"), Spec.Raw(0x07)]));
        cases.Add(new Case("one_byte_each", Spec.Expand(Osc("9;4;1;42", st: true)).Select(b => Spec.Raw(b)).ToArray()));

        var latestProp = LunegitTypes.Property(t.ProgressScanner, "Latest");
        Run(path, "osc-progress", header, ["state", "percent"], cases, () =>
        {
            var (feed, scanner) = Create(t.ProgressScanner);
            return (feed, () =>
            {
                object latest = latestProp.GetValue(scanner)!;
                var type = latest.GetType();
                string state = LunegitTypes.Property(type, "State").GetValue(latest)!.ToString()!;
                int percent = Convert.ToInt32(LunegitTypes.Property(type, "Percent").GetValue(latest), CultureInfo.InvariantCulture);
                return [state, percent.ToString(CultureInfo.InvariantCulture)];
            });
        });
    }

    // ---- command marks (OSC 133, CSI ?2026h) ----

    public static void WriteCommand(string path, List<(string, string)> header, LunegitTypes t)
    {
        var cases = new List<Case>();
        cases.AddRange(One(
        [
            ("mark_a", "133;A"),
            ("mark_b", "133;B"),
            ("mark_c", "133;C"),
            ("mark_d", "133;D"),
            ("mark_d_0", "133;D;0"),
            ("mark_d_255", "133;D;255"),
            ("mark_d_alpha", "133;D;abc"),
            ("mark_ab", "133;AB"),
            ("mark_empty", "133;"),
            ("mark_bare", "133"),
            ("mark_double_semi", "133;;"),
            ("mark_a_kitty_options", "133;A;special_key=1;click_events=1"),
            ("mark_c_options", "133;C;extra=1"),
            ("mark_lower_c", "133;c"),
            ("mark_e", "133;E;cmd"),
            ("mark_134", "134;C"),
            ("mark_13", "13;C"),
            ("mark_1330", "1330;C"),
            ("mark_leading_semi", ";C"),
            ("mark_c_semi_end", "133;C;"),
        ], alsoSt: true));
        cases.Add(new Case("body_exactly_64", [OscPad("133;C;", 64)]));
        cases.Add(new Case("body_65", [OscPad("133;C;", 65)]));
        cases.Add(new Case("body_64_st", [OscPad("133;C;", 64, st: true)]));
        cases.Add(new Case("body_65_st", [OscPad("133;C;", 65, st: true)]));
        cases.Add(new Case("body_300", [OscPad("133;C;", 300)]));
        cases.Add(new Case("overflow_then_c", [OscPad("133;C;", 65), Osc("133;C")]));
        cases.Add(new Case("flow_acd", [Osc("133;A"), Osc("133;B"), Osc("133;C"), Osc("133;D;0")]));
        cases.Add(new Case("flow_c_twice", [Osc("133;C"), Osc("133;C")]));
        cases.Add(new Case("flow_d_without_c", [Osc("133;D;1")]));
        cases.Add(new Case("flow_c_then_a", [Osc("133;C"), Osc("133;A")]));
        cases.Add(new Case("flow_c_then_b", [Osc("133;C"), Osc("133;B")]));
        cases.Add(new Case("flow_c_then_unknown", [Osc("133;C"), Osc("133;Z"), Osc("133;AB"), Osc("0;t")]));
        cases.Add(new Case("flow_three_commands", [Osc("133;C"), Osc("133;D"), Osc("133;C"), Osc("133;D"), Osc("133;C")]));
        cases.Add(new Case("flow_one_feed", [Spec.Cat(Osc("133;A"), Osc("133;C"), Osc("133;D"), Osc("133;C"))]));
        cases.Add(new Case("flow_garbage_before", [Spec.Cat(Csi("1mhello"), Osc("133;C"))]));
        cases.Add(new Case("flow_split_bytes", [Spec.Raw(0x1B), Spec.Lit("]13"), Spec.Lit("3;"), Spec.Lit("C"), Spec.Raw(0x1B), Spec.Raw(0x5C)]));

        // The 2026 latch: CSI ? 2026 h before and after C, and the parameter forms around it.
        void Sync(string name, string seq) =>
            cases.Add(new Case("sync_" + name, [Osc("133;C"), Csi(seq), Osc("133;D")]));

        Sync("plain", "?2026h");
        Sync("reset_l", "?2026l");
        Sync("list_first", "?2026;1h");
        Sync("list_last", "?1;2026h");
        Sync("list_mid", "?1;2026;3h");
        Sync("list_empty_first", "?;2026h");
        Sync("list_trailing_semi", "?2026;h");
        Sync("colon_after", "?2026:1h");
        Sync("colon_inside", "?202:6h");
        Sync("colon_before", "?:2026h");
        Sync("wraparound", "?4294969322h");
        Sync("leading_zero", "?02026h");
        Sync("digit_suffix", "?20260h");
        Sync("digit_prefix", "?12026h");
        Sync("non_private", "2026h");
        Sync("gt_prefix", ">2026h");
        Sync("lt_prefix", "<2026h");
        Sync("eq_prefix", "=2026h");
        Sync("question_late", "1;?2026h");
        Sync("intermediate", "?2026 h");
        Sync("intermediate_final_p", "?2026$p");
        Sync("final_other", "?2026c");
        Sync("long_list", "?1;2;3;4;5;6;7;8;9;10;2026h");
        Sync("guard_100000", "?100000;2026h");
        Sync("guard_100001", "?100001;2026h");
        Sync("huge_then_2026", "?99999999999;2026h");
        Sync("mode_2025", "?2025h");
        Sync("mode_2027", "?2027h");
        Sync("two_2026", "?2026;2026h");
        Sync("sgr_like", "?25h");
        cases.Add(new Case("sync_before_c", [Csi("?2026h"), Osc("133;C")]));
        cases.Add(new Case("sync_before_c_then_after", [Csi("?2026h"), Osc("133;C"), Csi("?2026h")]));
        cases.Add(new Case("sync_after_c_then_a", [Osc("133;C"), Csi("?2026h"), Osc("133;A")]));
        cases.Add(new Case("sync_after_c_then_c", [Osc("133;C"), Csi("?2026h"), Osc("133;C")]));
        cases.Add(new Case("sync_after_c_then_b", [Osc("133;C"), Csi("?2026h"), Osc("133;B")]));
        cases.Add(new Case("sync_without_c", [Csi("?2026h"), Csi("?2026h")]));
        cases.Add(new Case("sync_then_reset_then_set", [Osc("133;C"), Csi("?2026h"), Csi("?2026l"), Csi("?2026h")]));
        cases.Add(new Case("sync_two_commands", [Osc("133;C"), Csi("?2026h"), Osc("133;D"), Osc("133;C"), Csi("?2026h")]));
        cases.Add(new Case("sync_one_feed", [Spec.Cat(Osc("133;C"), Csi("?2026h"), Osc("133;D"), Osc("133;C"))]));
        cases.Add(new Case("sync_split_csi", [Osc("133;C"), Spec.Raw(0x1B), Spec.Lit("[?20"), Spec.Lit("26"), Spec.Lit("h")]));
        cases.Add(new Case("sync_split_param", [Osc("133;C"), Csi("?1;20"), Spec.Lit("26;3h")]));
        cases.Add(new Case("sync_one_byte_each", Spec.Expand(Spec.Cat(Osc("133;C"), Csi("?2026h"))).Select(b => Spec.Raw(b)).ToArray()));
        cases.Add(new Case("sync_inside_osc_body", [Osc("133;C"), Spec.Cat(Spec.Raw(0x1B, 0x5D), Spec.Lit("0;t"), Spec.Raw(0x07), Csi("?2026h"))]));
        cases.Add(new Case("sync_osc_between_csi", [Osc("133;C"), Csi("?20"), Osc("0;title"), Spec.Lit("26h")]));

        Run(path, "osc-command", header, ["running", "sync", "starts"], cases, () =>
        {
            var (feed, scanner) = Create(t.CommandScanner);
            var running = LunegitTypes.Property(t.CommandScanner, "IsCommandRunning");
            var sync = LunegitTypes.Property(t.CommandScanner, "SawSynchronizedUpdateSinceCommandStart");
            var starts = LunegitTypes.Property(t.CommandScanner, "CommandStartCount");
            return (feed, () =>
            [
                (bool)running.GetValue(scanner)! ? "1" : "0",
                (bool)sync.GetValue(scanner)! ? "1" : "0",
                ((int)starts.GetValue(scanner)!).ToString(CultureInfo.InvariantCulture),
            ]);
        });
    }
}
