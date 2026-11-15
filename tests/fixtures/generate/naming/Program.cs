// Executes FileNameBuilder from the real Tanukarr image. Proxies supply metadata only;
// token parsing, regular expressions and rendering are the original compiled implementation.
using System.Reflection;
using System.Runtime.Loader;
using System.Text.Json;
using NzbDrone.Common.Cache;
using NzbDrone.Core.CustomFormats;
using NzbDrone.Core.LibraryTools;
using NzbDrone.Core.MediaFiles;
using NzbDrone.Core.MediaFiles.MediaInfo;
using NzbDrone.Core.MetadataSource;
using NzbDrone.Core.Organizer;
using NzbDrone.Core.Qualities;
using NzbDrone.Core.Tv;

AssemblyLoadContext.Default.Resolving += (_, name) => {
    var path = Path.Combine("/reference-app", name.Name + ".dll");
    return File.Exists(path) ? AssemblyLoadContext.Default.LoadFromAssemblyPath(path) : null;
};
Run(args);

static void Run(string[] args)
{
    var results = new List<object>();
    var formats = new List<CustomFormat> {
        new() { Name = "INTERNAL", IncludeCustomFormatWhenRenaming = true },
        new() { Name = "AMZN", IncludeCustomFormatWhenRenaming = true },
        new() { Name = "NAME WITH SPACES", IncludeCustomFormatWhenRenaming = true },
        new() { Name = "Excluded", IncludeCustomFormatWhenRenaming = false }
    };
    void Check(string kind, string pattern, int style = 5, int count = 1, string title = "South Park", int version = 1)
    {
        var naming = NamingConfig.Default;
        naming.RenameEpisodes = true;
        naming.StandardEpisodeFormat = naming.DailyEpisodeFormat = naming.AnimeEpisodeFormat = naming.MovieFormat = pattern;
        naming.MultiEpisodeStyle = (MultiEpisodeStyle)style;
        var builder = new FileNameBuilder(
            Proxy<INamingConfigService>((_, _) => naming),
            Proxy<IQualityDefinitionService>((m, a) => m.Name == "Get" ? Quality.DefaultQualityDefinitions.First(d => d.Quality == (Quality)a[0]) : throw new Exception(m.Name)),
            new CacheManager(),
            Proxy<IUpdateMediaInfo>((m, _) => throw new Exception("Unexpected media-info update: " + m.Name)),
            Proxy<ICustomFormatCalculationService>((m, _) => throw new Exception("Unexpected format evaluation: " + m.Name)),
            Proxy<IProOptionsService>((m, _) => Activator.CreateInstance(m.ReturnType)),
            Proxy<IEpisodeService>((m, _) => throw new Exception("Unexpected episode lookup: " + m.Name)),
            NLog.LogManager.CreateNullLogger());
        var series = new Series {
            Id = 1, Title = title, Year = 2010, Path = "/library/show", SeasonFolder = true,
            SeriesType = kind == "anime" ? SeriesTypes.Anime : kind == "daily" ? SeriesTypes.Daily : SeriesTypes.Standard,
            MediaType = kind == "movie" ? MediaType.Movie : MediaType.Series,
            MetadataSource = kind == "movie" ? MetadataSourceType.TmdbMovie : MetadataSourceType.Tvdb,
        };
        var episodes = Enumerable.Range(1, count).Select(n => new Episode {
            SeasonNumber = 1, EpisodeNumber = n, AbsoluteEpisodeNumber = 100 + n,
            Title = count == 1 ? "City Sushi" : $"The Story ({n})", AirDate = "2010-01-02"
        }).ToList();
        var file = new EpisodeFile {
            Id = 1, RelativePath = "original.mkv", SceneName = "South.Park.S01E01.720p.HDTV.x264-GRP",
            ReleaseGroup = "GRP", Quality = new QualityModel(Quality.HDTV720p, new Revision(version))
        };
        var expected = builder.BuildFileName(episodes, series, file, ".mkv", naming, formats);
        results.Add(new { kind, pattern, style, count, title, version, expected });
    }
    var standard = new[] {
        "{Series Title} - S{season:00}E{episode:00} - {Episode Title} [{Quality Full}]-{Release Group}",
        "{Series.Title}.S{season:00}E{episode:00}.{Episode.Title}.{Quality.Full}",
        "{SERIES TITLE} {season}x{episode:00}", "{series_title} {season:00}x{episode}",
        "{Series Title} S{season:00}E{episode:00} {S00E00}",
        "S{season:00}E{episode:00} - {Episode CleanTitle}",
        "{Series Title}/S{season:00}E{episode:00} - {Episode Title}",
        "{Series Title} -- S{season:00}E{episode:00}__{Episode Title}...",
        "{{{Series Title}}} S{season:00}E{episode:00} {[Quality Proper]} {[-Release Group]}",
        "{Original Title}", "{Original Filename}",
        "{Series Title} S{season:00}E{episode:00} {[Custom Formats]}",
        "{Series Title} S{season:00}E{episode:00} {[Custom Formats:-INTERNAL,NAME WITH SPACES]}",
        "{Series Title} S{season:00}E{episode:00} {[Custom Format:INTERNAL]}",
        "{Series Title} S{season:00}E{episode:00} {Custom Formats:}",
        "{Series Title} S{season:00}E{episode:00} {Custom Formats:-}",
    };
    foreach (var pattern in standard)
    foreach (var style in Enumerable.Range(0, 6))
    foreach (var count in new[] { 1, 2 }) Check("standard", pattern, style, count);
    foreach (var pattern in new[] {
        "{Series Title} {Air-Date} - {Episode Title}", "{Series.Title}.{air_date}.{Quality.Full}",
        "{Series Title} {Air Date} {[Custom Formats]}", "{Series Title} S{season:00}E{episode:00}"
    }) foreach (var count in new[] { 1, 2 }) Check("daily", pattern, count: count);
    foreach (var pattern in new[] {
        "{Series Title} - {absolute:000} - {Episode Title} [{Quality Full}]",
        "[{Release Group}] {Series.Title} {absolute:000} {[Quality Proper]}",
        "{Series Title} S{season:00}E{episode:00} ({absolute:000})"
    }) foreach (var style in Enumerable.Range(0, 6))
    foreach (var count in new[] { 1, 2 }) Check("anime", pattern, style, count, version: 2);
    foreach (var title in new[] { "The Show (2010)", "An Adventure", "A Test", "A/B: C & D's Show", "My {Quality Full} Title", "探偵の物語", "¿Qué tal?" })
    foreach (var pattern in new[] { "{Series Title}", "{Series TitleThe}", "{Series CleanTitle}", "{Series CleanTitleTheYear}", "{Series TitleWithoutYear}", "{Series Title:12}", "{Series Title:-12}", "{{{Series Title}}}" })
        Check("standard", pattern, title: title);
    foreach (var pattern in new[] {
        "{Series Title} ({Series Year}) [{Quality Full}]-{Release Group}",
        "{Series.TitleYear}.{Quality.Title}", "{SERIES TITLE} ({Series Year})",
        "{Series TitleTheYear}", "{Series CleanTitle}", "{Original Title}",
        "{Series Title} {[Custom Formats]}", "{Series Title} {[Custom Formats:-INTERNAL]}",
        "{Series Title} {[Quality Proper]}", "{Series Title} {Custom Format:AMZN}"
    }) Check("movie", pattern, version: 2);
    var regexCases = new List<object>();
    foreach (var pattern in new[] {
        @"(?<=\.)x26[45](?=\.)", @"\b(?<codec>x265|hevc)\b", @"(?i:hevc)",
        @"\b(?<word>dual)[ ._-]+\k<word>\b", @"(?<!not[ ._-])hevc",
        @"(?:(?<word>dual)|(?<word>multi))[ ._-]+\k<word>"
    })
    foreach (var title in new[] { "Show.S01E01.x265.GRP", "HEVC", "not-HEVC", "dual dual", "multi_multi", "none" })
    foreach (var negate in new[] { false, true })
    {
        var filename = title == "none" ? "Show.HEVC.mkv" : "unchanged.mkv";
        var spec = new ReleaseTitleSpecification { Value = pattern, Negate = negate };
        var input = new CustomFormatInput { EpisodeInfo = new NzbDrone.Core.Parser.Model.ParsedEpisodeInfo { ReleaseTitle = title }, Filename = filename };
        regexCases.Add(new { pattern, title, filename, negate, matched = spec.IsSatisfiedBy(input) });
    }
    File.WriteAllText(args[0], JsonSerializer.Serialize(new { image = "ghcr.io/kvasfan/tanukarr:0.1.5", assemblyVersion = typeof(FileNameBuilder).Assembly.GetName().Version.ToString(), cases = results, regexCases }, new JsonSerializerOptions { WriteIndented = true }));
    Console.WriteLine($"Wrote {results.Count} actual Tanukarr filename results.");
}

static T Proxy<T>(Func<MethodInfo, object[], object> callback) where T : class
{
    var service = DispatchProxy.Create<T, ServiceProxy>();
    ((ServiceProxy)(object)service).Callback = callback;
    return service;
}
public class ServiceProxy : DispatchProxy
{
    public Func<MethodInfo, object[], object> Callback;
    protected override object Invoke(MethodInfo method, object[] args) => Callback(method, args);
}
