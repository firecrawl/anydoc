using System.Runtime.InteropServices;

namespace Firecrawl.Anydoc.Native
{
    /// <summary>
    /// DllImport surface, extended with the platform-specific loader that maps
    /// the logical library name to <c>runtimes/{platform}-{arch}/native/</c>,
    /// the layout both NuGet and this project's build output use.
    /// </summary>
    internal static unsafe partial class AnydocNative
    {
        static AnydocNative()
        {
            NativeLibrary.SetDllImportResolver(typeof(AnydocNative).Assembly, ResolveLibrary);
        }

        private static nint ResolveLibrary(string libraryName, System.Reflection.Assembly assembly, DllImportSearchPath? searchPath)
        {
            if (libraryName != __DllName)
            {
                return nint.Zero;
            }

            string platform = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "win"
                : RuntimeInformation.IsOSPlatform(OSPlatform.OSX) ? "osx"
                : RuntimeInformation.IsOSPlatform(OSPlatform.Linux) ? "linux"
                : throw new PlatformNotSupportedException($"anydoc has no native library for {RuntimeInformation.OSDescription}");

            string arch = RuntimeInformation.OSArchitecture switch
            {
                Architecture.X64 => "x64",
                Architecture.Arm64 => "arm64",
                _ => throw new PlatformNotSupportedException($"anydoc has no native library for {RuntimeInformation.OSArchitecture}"),
            };

            string name = libraryName;
            string prefix = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? "" : "lib";
            string ext = RuntimeInformation.IsOSPlatform(OSPlatform.Windows) ? ".dll"
                : RuntimeInformation.IsOSPlatform(OSPlatform.OSX) ? ".dylib"
                : ".so";

            string relative = $"runtimes/{platform}-{arch}/native/{prefix}{name}{ext}";
            foreach (string baseDir in new[] { AppContext.BaseDirectory, Path.GetDirectoryName(typeof(AnydocNative).Assembly.Location) ?? "" })
            {
                if (string.IsNullOrEmpty(baseDir))
                {
                    continue;
                }
                string candidate = Path.Combine(baseDir, relative);
                if (File.Exists(candidate))
                {
                    return NativeLibrary.Load(candidate, assembly, searchPath);
                }
            }
            // Last chance: let the default resolution attempt it (NuGet hosts
            // native libs on the default search path too).
            return NativeLibrary.Load(relative, assembly, searchPath);
        }
    }
}
