// Differential oracle using the original Tanukarr implementation, not a translation.
using System.Text.Json;
using NzbDrone.Core.Configuration;
using NzbDrone.Core.CustomFormats;
using NzbDrone.Core.DecisionEngine.Specifications;
using NzbDrone.Core.Profiles;
using NzbDrone.Core.Profiles.Qualities;
using NzbDrone.Core.Qualities;

var qualities = new[] { 1, 9, 3, 7 }.ToDictionary(id => id, id => new Quality { Id = id });
var revisions = new[] { new[] { 1, 0, 1, 0 }, new[] { 1, 0, 2, 0 }, new[] { 2, 0, 1, 0 }, new[] { 2, 0, 1, 1 } };
var scores = new[] { new[] { -50, 0 }, new[] { 0, 0 }, new[] { 0, 25 }, new[] { 0, 50 }, new[] { 50, 100 }, new[] { 100, 150 }, new[] { 0, -50 } };
var profiles = new List<object>();
foreach (var upgrades in new[] { false, true })
foreach (var cutoff in new[] { 1000, 7 })
{
    var profile = new QualityProfile {
        Name = "Reference", UpgradeAllowed = upgrades, Cutoff = cutoff,
        MinFormatScore = 0, CutoffFormatScore = 100, MinUpgradeFormatScore = 50,
        Items = new() {
            new() { Quality = qualities[1], Allowed = false },
            new() { Id = 1000, Name = "HD group", Allowed = true, Items = new() {
                new() { Quality = qualities[9], Allowed = true },
                new() { Quality = qualities[3], Allowed = true } } },
            new() { Quality = qualities[7], Allowed = true }
        }
    };
    var results = new List<string>();
    foreach (var currentId in qualities.Keys)
    foreach (var newId in qualities.Keys)
    foreach (var revision in revisions)
    foreach (var score in scores)
    foreach (var proper in Enum.GetValues<ProperDownloadTypes>())
    {
        var config = new Config { DownloadPropersAndRepacks = proper };
        var spec = new UpgradableSpecification(config, new NLog.Logger());
        var current = new QualityModel { Quality = qualities[currentId], Revision = new Revision(revision[0], revision[1]) };
        var next = new QualityModel { Quality = qualities[newId], Revision = new Revision(revision[2], revision[3]) };
        var currentFormats = new List<CustomFormat> { new() { Id = 1 } };
        var newFormats = new List<CustomFormat> { new() { Id = 2 } };
        profile.FormatItems = new() { new() { Format = currentFormats[0], Score = score[0] }, new() { Format = newFormats[0], Score = score[1] } };
        results.Add(string.Join("|",
            Math.Sign(new QualityModelComparer(profile).Compare(next, current)),
            spec.QualityCutoffNotMet(profile, current, next),
            spec.CutoffNotMet(profile, current, currentFormats, next),
            spec.IsRevisionUpgrade(current, next),
            spec.IsUpgradeAllowed(profile, current, currentFormats, next, newFormats),
            spec.IsUpgradable(profile, current, currentFormats, next, newFormats)));
    }
    profiles.Add(new { upgrades, cutoff, results });
}
File.WriteAllText(args[0], JsonSerializer.Serialize(new {
    sourceRevision = args[1], qualityIds = qualities.Keys, revisions, scores,
    resultColumns = "compareModels|qualityCutoffNotMet|cutoffNotMet|isRevisionUpgrade|isUpgradeAllowed|isUpgradable",
    profiles
}));
Console.WriteLine($"Wrote {profiles.Count * 16 * revisions.Length * scores.Length * 3} Tanukarr reference cases.");
