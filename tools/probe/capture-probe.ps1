# Which kinds of overlay window can Windows hide from screen capture
# (SetWindowDisplayAffinity WDA_EXCLUDEFROMCAPTURE), and does BitBlt then
# really not see them? Run on Windows 10 and 11 runners (probe.yml).
$ErrorActionPreference = "Stop"
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Runtime.InteropServices;

public static class Probe {
    delegate IntPtr WndProc(IntPtr h, uint m, IntPtr w, IntPtr l);
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct WNDCLASS { public uint style; public WndProc proc; public int ce; public int we; public IntPtr inst; public IntPtr icon; public IntPtr cursor; public IntPtr brush; public string menu; public string cls; }
    [StructLayout(LayoutKind.Sequential)] struct POINT { public int x, y; }
    [StructLayout(LayoutKind.Sequential)] struct SIZE { public int cx, cy; }
    [StructLayout(LayoutKind.Sequential)] struct BLEND { public byte op, flags, alpha, fmt; }
    [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr h; public uint m; public IntPtr w, l; public uint t; public POINT p; }
    [StructLayout(LayoutKind.Sequential)] struct BMI { public int size, w, h; public short planes, bits; public int comp, img, xp, yp, used, imp; }

    [DllImport("user32", CharSet = CharSet.Unicode, SetLastError = true)] static extern ushort RegisterClassW(ref WNDCLASS c);
    [DllImport("user32", CharSet = CharSet.Unicode, SetLastError = true)] static extern IntPtr CreateWindowExW(uint ex, string cls, string name, uint style, int x, int y, int w, int h, IntPtr p, IntPtr m, IntPtr i, IntPtr param);
    [DllImport("user32")] static extern IntPtr DefWindowProcW(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32")] static extern bool ShowWindow(IntPtr h, int c);
    [DllImport("user32")] static extern bool DestroyWindow(IntPtr h);
    [DllImport("user32")] static extern bool UpdateWindow(IntPtr h);
    [DllImport("user32")] static extern bool PeekMessageW(out MSG m, IntPtr h, uint a, uint b, uint r);
    [DllImport("user32")] static extern bool TranslateMessage(ref MSG m);
    [DllImport("user32")] static extern IntPtr DispatchMessageW(ref MSG m);
    [DllImport("user32", SetLastError = true)] static extern bool SetWindowDisplayAffinity(IntPtr h, uint a);
    [DllImport("user32", SetLastError = true)] static extern bool SetLayeredWindowAttributes(IntPtr h, uint key, byte a, uint f);
    [DllImport("user32", SetLastError = true)] static extern bool UpdateLayeredWindow(IntPtr h, IntPtr dst, ref POINT pd, ref SIZE s, IntPtr src, ref POINT ps, uint key, ref BLEND b, uint f);
    [DllImport("user32")] static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32")] static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("gdi32")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32")] static extern IntPtr CreateSolidBrush(uint c);
    [DllImport("gdi32")] static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
    [DllImport("gdi32")] static extern IntPtr CreateDIBSection(IntPtr dc, ref BMI b, uint u, out IntPtr bits, IntPtr s, uint o);
    [DllImport("kernel32")] static extern IntPtr GetModuleHandleW(IntPtr n);

    static WndProc proc = (h, m, w, l) => DefWindowProcW(h, m, w, l);
    const uint RED = 0x0000FF; // COLORREF 0x00BBGGRR
    const uint WS_POPUP = 0x80000000;
    const uint EX_LAYERED = 0x80000, EX_TRANSPARENT = 0x20, EX_TOPMOST = 0x8, EX_TOOL = 0x80, EX_NOACTIVATE = 0x8000000, EX_NOREDIR = 0x200000;

    static void Pump(int ms) {
        var end = DateTime.Now.AddMilliseconds(ms);
        while (DateTime.Now < end) {
            MSG m;
            while (PeekMessageW(out m, IntPtr.Zero, 0, 0, 1)) { TranslateMessage(ref m); DispatchMessageW(ref m); }
            System.Threading.Thread.Sleep(10);
        }
    }

    static bool Red(int x, int y) {
        using (var bmp = new Bitmap(1, 1))
        using (var g = Graphics.FromImage(bmp)) {
            g.CopyFromScreen(x, y, 0, 0, new Size(1, 1));
            var c = bmp.GetPixel(0, 0);
            return c.R > 200 && c.G < 60 && c.B < 60;
        }
    }

    static void Ulw(IntPtr h, int x, int y, int w, int hh) {
        var bmi = new BMI { size = 40, w = w, h = -hh, planes = 1, bits = 32 };
        IntPtr bits;
        var scr = GetDC(IntPtr.Zero);
        var dc = CreateCompatibleDC(scr);
        var dib = CreateDIBSection(dc, ref bmi, 0, out bits, IntPtr.Zero, 0);
        SelectObject(dc, dib);
        var px = new byte[w * hh * 4];
        for (int i = 0; i < px.Length; i += 4) { px[i] = 0; px[i + 1] = 0; px[i + 2] = 255; px[i + 3] = 255; }
        Marshal.Copy(px, 0, bits, px.Length);
        var pd = new POINT { x = x, y = y }; var ps = new POINT(); var s = new SIZE { cx = w, cy = hh };
        var b = new BLEND { op = 0, flags = 0, alpha = 255, fmt = 1 };
        if (!UpdateLayeredWindow(h, scr, ref pd, ref s, dc, ref ps, 0, ref b, 2))
            Console.WriteLine("   UpdateLayeredWindow error " + Marshal.GetLastWin32Error());
        ReleaseDC(IntPtr.Zero, scr);
    }

    public static void Run() {
        Console.WriteLine("Windows " + Environment.OSVersion.Version);
        var inst = GetModuleHandleW(IntPtr.Zero);
        var wc = new WNDCLASS { proc = proc, inst = inst, brush = CreateSolidBrush(RED), cls = "WuxiaProbe" };
        RegisterClassW(ref wc);
        string[] kinds = { "plain popup", "layered + UpdateLayeredWindow (now)", "layered + affinity before ULW", "layered + SetLayeredWindowAttributes colour key", "layered + SetLayeredWindowAttributes alpha 255", "layered + NOREDIRECTIONBITMAP" };
        for (int k = 0; k < kinds.Length; k++) {
            int x = 200 + (k % 3) * 220, y = 200 + (k / 3) * 160, w = 160, hh = 100;
            uint ex = EX_TOPMOST | EX_TOOL | EX_NOACTIVATE;
            if (k > 0) ex |= EX_LAYERED | EX_TRANSPARENT;
            if (k == 5) ex |= EX_NOREDIR;
            var h = CreateWindowExW(ex, "WuxiaProbe", "probe", WS_POPUP, x, y, w, hh, IntPtr.Zero, IntPtr.Zero, inst, IntPtr.Zero);
            string early = "";
            if (k == 2) early = SetWindowDisplayAffinity(h, 0x11) ? "ok" : "error " + Marshal.GetLastWin32Error();
            if (k == 1 || k == 2) Ulw(h, x, y, w, hh);
            if (k == 3) SetLayeredWindowAttributes(h, 0xFF00FF, 255, 1);
            if (k == 4) SetLayeredWindowAttributes(h, 0, 255, 2);
            ShowWindow(h, 4);
            UpdateWindow(h);
            Pump(700);
            bool before = Red(x + w / 2, y + hh / 2);
            string aff = k == 2 ? early : (SetWindowDisplayAffinity(h, 0x11) ? "ok" : "error " + Marshal.GetLastWin32Error());
            Pump(700);
            bool after = Red(x + w / 2, y + hh / 2);
            Console.WriteLine(String.Format("{0,-50} seen before: {1,-5} affinity: {2,-10} seen after: {3,-5} => {4}",
                kinds[k], before, aff, after, before && !after ? "HIDDEN FROM CAPTURE" : (before ? "still captured" : "not visible at all")));
            DestroyWindow(h);
            Pump(200);
        }
    }
}
'@
[Probe]::Run()
