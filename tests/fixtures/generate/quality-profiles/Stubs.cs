// Infrastructure only. All profile, ordering, revision and upgrade algorithms are linked
// unmodified from Tanukarr by Reference.csproj; no NuGet packages or services are required.
namespace NLog { public class Logger { public void Debug(string message, params object[] args) {} } }
namespace NzbDrone.Common.Extensions
{
    public static class Extensions
    {
        public static string ConcatToString<T>(this IEnumerable<T> values) => string.Join(",", values);
        public static bool IsNotNullOrWhiteSpace(this string value) => !string.IsNullOrWhiteSpace(value);
    }
}
namespace NzbDrone.Common.EnsureThat
{
    public static class Ensure { public static Check That(object value, System.Linq.Expressions.Expression<Func<object>> name) => new(); }
    public class Check { public void IsNotNull() {} public void HasItems() {} }
}
namespace NzbDrone.Core.Datastore
{
    public class ModelBase { public int Id { get; set; } }
    public interface IEmbeddedDocument {}
}
namespace NzbDrone.Core.CustomFormats { public class CustomFormat { public int Id { get; set; } } }
namespace NzbDrone.Core.Configuration
{
    public interface IConfigService { NzbDrone.Core.Qualities.ProperDownloadTypes DownloadPropersAndRepacks { get; } }
    public class Config : IConfigService { public NzbDrone.Core.Qualities.ProperDownloadTypes DownloadPropersAndRepacks { get; set; } }
}
namespace NzbDrone.Core.Qualities
{
    public class Quality { public int Id { get; set; } }
    public class QualityModel { public Quality Quality { get; set; } public Revision Revision { get; set; } }
    public class CodecMultiplierOverrides : Dictionary<string, decimal> {}
}
