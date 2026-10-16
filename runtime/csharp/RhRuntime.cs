using System;
using System.Collections;
using System.Collections.Generic;
using System.Linq;

namespace Roundhouse;

// Module-level constants from the transpiled framework runtime (json_builder's
// ESCAPES, router's STATUS_CODES, …) have no top-level form in C#, so the
// emitter appends them as fragments of this partial class. This empty base
// declaration guarantees the type always exists, so the `using static
// Roundhouse.RuntimeConstants` in each runtime file resolves even when a file
// (or the whole runtime) carries no constants.
public static partial class RuntimeConstants { }

// Small bridging helpers the emitter targets for a few Ruby idioms that have
// no direct C# operator: `Hash#merge` and the `rescue` modifier.
public static class RhRuntime
{
    // Ruby's `str[start, length]`: a negative start counts from the end,
    // and the slice is CLAMPED to the string (`"abc"[1, 10]` is "bc"),
    // where `Substring` throws. Ruby answers nil for a start past the
    // end; the emitted call is typed string, so that case answers "".
    public static string StrSlice(string s, long start, long length)
    {
        long n = s.Length;
        if (start < 0) start += n;
        if (start < 0 || start > n || length < 0) return "";
        return s.Substring((int)start, (int)Math.Min(length, n - start));
    }

    // `a.merge(b)` → a new dictionary, b winning on key collisions.
    public static Dictionary<string, object?> Merge(object? a, object? b)
    {
        var result = new Dictionary<string, object?>();
        Absorb(result, a);
        Absorb(result, b);
        return result;
    }

    private static void Absorb(Dictionary<string, object?> into, object? src)
    {
        if (src is IDictionary dict)
        {
            foreach (DictionaryEntry e in dict)
            {
                into[Convert.ToString(e.Key) ?? ""] = e.Value;
            }
        }
    }

    // `expr rescue fallback` — evaluate `body`, falling back on any exception.
    public static T Rescue<T>(Func<T> body, Func<T> fallback)
    {
        try { return body(); }
        catch { return fallback(); }
    }
}

// An `is_a?(Array)` binding over any list. Not a copy: Ruby's `value << x` in the branch must reach the original, whatever its element type.
public sealed class RhArrayView : IList<object?>, IList
{
    readonly IList inner;
    public RhArrayView(IList inner) { this.inner = inner; }
    public object? this[int index] { get => inner[index]; set => inner[index] = value; }
    public int Count => inner.Count;
    public bool IsReadOnly => inner.IsReadOnly;
    public void Add(object? item) => inner.Add(item);
    public void Clear() => inner.Clear();
    public bool Contains(object? item) => inner.Contains(item);
    public void CopyTo(object?[] array, int arrayIndex) => inner.CopyTo(array, arrayIndex);
    public int IndexOf(object? item) => inner.IndexOf(item);
    public void Insert(int index, object? item) => inner.Insert(index, item);
    public void RemoveAt(int index) => inner.RemoveAt(index);
    public bool Remove(object? item)
    {
        int i = inner.IndexOf(item);
        if (i < 0) return false;
        inner.RemoveAt(i);
        return true;
    }
    public IEnumerator<object?> GetEnumerator()
    {
        foreach (var e in inner) yield return e;
    }
    IEnumerator IEnumerable.GetEnumerator() => inner.GetEnumerator();
    int IList.Add(object? item) => inner.Add(item);
    void IList.Remove(object? item) => inner.Remove(item);
    bool IList.IsFixedSize => inner.IsFixedSize;
    void ICollection.CopyTo(Array array, int index) => inner.CopyTo(array, index);
    bool ICollection.IsSynchronized => inner.IsSynchronized;
    object ICollection.SyncRoot => inner.SyncRoot;
}

// An `is_a?(Hash)` binding over any dictionary. Not a copy: Ruby's `value[k] = v` in the branch must reach the original, whatever its key and value types.
public sealed class RhHashView : IDictionary<object, object?>, IReadOnlyDictionary<object, object?>, IDictionary
{
    readonly IDictionary inner;
    public RhHashView(IDictionary inner) { this.inner = inner; }
    public object? this[object key] { get => inner[key]; set => inner[key] = value; }
    public ICollection<object> Keys => inner.Keys.Cast<object>().ToList();
    public ICollection<object?> Values => inner.Values.Cast<object?>().ToList();
    IEnumerable<object> IReadOnlyDictionary<object, object?>.Keys => Keys;
    IEnumerable<object?> IReadOnlyDictionary<object, object?>.Values => Values;
    public int Count => inner.Count;
    public bool IsReadOnly => inner.IsReadOnly;
    public void Add(object key, object? value) => inner.Add(key, value);
    public void Add(KeyValuePair<object, object?> item) => inner.Add(item.Key, item.Value);
    public void Clear() => inner.Clear();
    public bool Contains(KeyValuePair<object, object?> item) => inner.Contains(item.Key) && Equals(inner[item.Key], item.Value);
    public bool ContainsKey(object key) => inner.Contains(key);
    public void CopyTo(KeyValuePair<object, object?>[] array, int arrayIndex)
    {
        foreach (var kv in this) array[arrayIndex++] = kv;
    }
    public bool Remove(object key)
    {
        if (!inner.Contains(key)) return false;
        inner.Remove(key);
        return true;
    }
    public bool Remove(KeyValuePair<object, object?> item) => Contains(item) && Remove(item.Key);
    public bool TryGetValue(object key, out object? value)
    {
        value = inner.Contains(key) ? inner[key] : null;
        return inner.Contains(key);
    }
    public object? GetValueOrDefault(object key) => inner.Contains(key) ? inner[key] : null;
    public object? GetValueOrDefault(object key, object? fallback) => inner.Contains(key) ? inner[key] : fallback;
    public IEnumerator<KeyValuePair<object, object?>> GetEnumerator()
    {
        foreach (DictionaryEntry e in inner) yield return new KeyValuePair<object, object?>(e.Key, e.Value);
    }
    IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();
    ICollection IDictionary.Keys => inner.Keys;
    ICollection IDictionary.Values => inner.Values;
    bool IDictionary.Contains(object key) => inner.Contains(key);
    void IDictionary.Remove(object key) => inner.Remove(key);
    bool IDictionary.IsFixedSize => inner.IsFixedSize;
    IDictionaryEnumerator IDictionary.GetEnumerator() => inner.GetEnumerator();
    void ICollection.CopyTo(Array array, int index) => inner.CopyTo(array, index);
    bool ICollection.IsSynchronized => inner.IsSynchronized;
    object ICollection.SyncRoot => inner.SyncRoot;
}
