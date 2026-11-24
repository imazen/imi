// ASP.NET Core server driving the same encoders through the C ABI in
// ../src/ffi.rs: GET /enc?codec=&w=&h=&mode=stream|buffer&strip=&src=photo|noise.
//
// stream: begin, then per strip push_rows (P/Invoke, on the request's thread),
// copy whatever is pending into Response.BodyWriter (PipeWriter.GetSpan /
// Advance) and await FlushAsync, the way a .NET host integrates a pull-bytes
// encoder. buffer: one-shot encode, Content-Length, one write.
//
// Usage: dotnet run -c Release -- <port> <photo> <path to libimij_stream_http>

using System.Runtime.InteropServices;
using System.Text;

var port = int.Parse(args[0]);
var photoPath = args[1];
NativeLibrary.SetDllImportResolver(typeof(Native).Assembly, (name, _, _) =>
    name == "imij_stream_http" ? NativeLibrary.Load(args[2]) : IntPtr.Zero);

var photo = Source.Load(photoPath);
var noise = Source.Load($"noise:{photo.Width}x{photo.Height}");
Console.Error.WriteLine($"photo {photo.Width}x{photo.Height}; listening on 0.0.0.0:{port}");

var builder = WebApplication.CreateSlimBuilder();
builder.Logging.ClearProviders();
builder.WebHost.UseUrls($"http://0.0.0.0:{port}");
var app = builder.Build();

app.MapGet("/enc", async (HttpContext ctx) =>
{
    var q = ctx.Request.Query;
    string codec = q["codec"].ToString();
    var src = q["src"] == "noise" ? noise : photo;
    int w = Math.Clamp(int.TryParse(q["w"], out var qw) ? qw : 1024, 1, src.Width);
    int h = Math.Clamp(int.TryParse(q["h"], out var qh) ? qh : 1024, 1, src.Height);
    int strip = Math.Max(1, int.TryParse(q["strip"], out var qs) ? qs : 16);
    bool stream = q["mode"] != "buffer";

    var s = Native.New(codec, w);
    if (s == IntPtr.Zero) { ctx.Response.StatusCode = 400; return; }
    try
    {
        ctx.Response.ContentType = "application/octet-stream";
        var writer = ctx.Response.BodyWriter;
        if (stream)
        {
            if (Native.sh_begin(s, (uint)w, (uint)h) < -1) { ctx.Response.StatusCode = 500; return; }
            await Drain(s, writer);
            for (int y = 0; y < h; y += strip)
            {
                int rows = Math.Min(strip, h - y);
                if (src.Push(s, w, y, rows) != 0) throw new InvalidOperationException("push_rows failed");
                await Drain(s, writer);
            }
            if (Native.sh_finish(s) != 0) throw new InvalidOperationException("finish failed");
            await Drain(s, writer);
        }
        else
        {
            if (src.Encode(s, w, h) != 0) { ctx.Response.StatusCode = 500; return; }
            ctx.Response.ContentLength = (long)Native.PendingLength(s);
            await Drain(s, writer);
        }
    }
    finally { Native.sh_free(s); }
});

app.Run();

// Copy everything pending into the pipe and flush it to the socket.
static async ValueTask Drain(IntPtr s, System.IO.Pipelines.PipeWriter writer)
{
    int n = Native.CopyPending(s, writer);
    if (n > 0) await writer.FlushAsync();
}

sealed unsafe class Source
{
    public int Width, Height;
    public byte[] Pixels = Array.Empty<byte>();

    public static Source Load(string path)
    {
        var p = Encoding.UTF8.GetBytes(path);
        uint w, h;
        nuint need;
        fixed (byte* pp = p) need = Native.sh_load(pp, (nuint)p.Length, null, 0, &w, &h);
        if (need == 0) throw new InvalidOperationException($"cannot load {path}");
        // Pinned: native code reads rows in place.
        var px = GC.AllocateUninitializedArray<byte>((int)need, pinned: true);
        fixed (byte* pp = p) fixed (byte* o = px) Native.sh_load(pp, (nuint)p.Length, o, need, &w, &h);
        return new Source { Width = (int)w, Height = (int)h, Pixels = px };
    }

    int Stride => Width * 3;

    // Rows y..y+rows of the top-left `w`-wide crop, read in place.
    public int Push(IntPtr s, int w, int y, int rows)
    {
        int start = y * Stride, len = (rows - 1) * Stride + w * 3;
        fixed (byte* p = &Pixels[start]) return Native.sh_push(s, p, (nuint)len, (uint)w, (uint)rows, (nuint)Stride);
    }

    public int Encode(IntPtr s, int w, int h)
    {
        int len = (h - 1) * Stride + w * 3;
        fixed (byte* p = Pixels) return Native.sh_encode(s, p, (nuint)len, (uint)w, (uint)h, (nuint)Stride);
    }
}

static unsafe partial class Native
{
    const string Lib = "imij_stream_http";
    [LibraryImport(Lib)] public static partial IntPtr sh_new(byte* codec, nuint len, uint width);
    [LibraryImport(Lib)] public static partial long sh_begin(IntPtr s, uint width, uint height);
    [LibraryImport(Lib)] public static partial int sh_push(IntPtr s, byte* px, nuint len, uint width, uint rows, nuint stride);
    [LibraryImport(Lib)] public static partial nuint sh_pending(IntPtr s, byte** outp);
    [LibraryImport(Lib)] public static partial void sh_consume(IntPtr s, nuint n);
    [LibraryImport(Lib)] public static partial int sh_finish(IntPtr s);
    [LibraryImport(Lib)] public static partial int sh_encode(IntPtr s, byte* px, nuint len, uint width, uint height, nuint stride);
    [LibraryImport(Lib)] public static partial void sh_free(IntPtr s);
    [LibraryImport(Lib)] public static partial nuint sh_load(byte* path, nuint len, byte* outp, nuint cap, uint* w, uint* h);

    public static IntPtr New(string codec, int w)
    {
        var b = Encoding.UTF8.GetBytes(codec);
        fixed (byte* p = b) return sh_new(p, (nuint)b.Length, (uint)w);
    }

    public static nuint PendingLength(IntPtr s)
    {
        byte* p;
        return sh_pending(s, &p);
    }

    // The single copy a pull-bytes encoder costs a PipeWriter host: native
    // buffer -> pipe memory.
    public static int CopyPending(IntPtr s, System.IO.Pipelines.PipeWriter writer)
    {
        byte* p;
        int n = (int)sh_pending(s, &p);
        if (n == 0) return 0;
        new ReadOnlySpan<byte>(p, n).CopyTo(writer.GetSpan(n));
        writer.Advance(n);
        sh_consume(s, (nuint)n);
        return n;
    }
}
