using System;
using System.IO;
using System.Threading;

namespace Roundhouse;

public static class I18nHost
{
    private sealed class Scope
    {
        public string Name = "";
    }

    // Not the name itself in the AsyncLocal: a value set inside an async before_action reverts when it returns, so each request holds one Scope it mutates
    private static readonly AsyncLocal<Scope?> Current = new();
    private static readonly Scope Outside = new();
    private static string? Table;

    public static void BeginRequest()
    {
        Current.Value = new Scope();
    }

    private static Scope Here() => Current.Value ?? Outside;

    public static string LocaleName() => Here().Name;

    public static string SetLocaleName(string name)
    {
        Here().Name = name;
        return name;
    }

    public static string EnvName() =>
        Environment.GetEnvironmentVariable("RAILS_ENV") ?? Environment.GetEnvironmentVariable("RACK_ENV") ?? "development";

    // Not the current directory alone: `dotnet test` runs from the test build's bin directory, below the app root
    public static string TableText()
    {
        if (Table != null) return Table;
        for (var dir = new DirectoryInfo(Directory.GetCurrentDirectory()); dir != null; dir = dir.Parent)
        {
            var path = Path.Combine(dir.FullName, "config", "i18n_translations");
            if (File.Exists(path)) return Table = File.ReadAllText(path);
        }
        throw new FileNotFoundException("config/i18n_translations is in no directory above " + Directory.GetCurrentDirectory());
    }
}
