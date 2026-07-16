#ifndef OSMFLAT_GROUP_HIERARCHY_HPP
#define OSMFLAT_GROUP_HIERARCHY_HPP

#include <mapnik/expression.hpp>
#include <mapnik/feature.hpp>

#include <cstdint>
#include <optional>
#include <set>
#include <string>
#include <utility>
#include <vector>

namespace osmflat {

/// One compiled `<Rule>` from a `group_hierarchy` XML file: a filter
/// expression plus the `/`-separated path it resolves to when the filter
/// matches (stored verbatim from the `path` attribute -- dumped as-is,
/// never split/rejoined here). `rank` is this rule's row index in the file:
/// the single source of truth for both match priority (rules are tried in
/// this order, first match wins) and draw z-order for the downstream SVG
/// post-processor.
struct group_rule {
    mapnik::expression_ptr filter;
    std::string path;
    int32_t rank;
};

/// Parses a file shaped like:
/// ```xml
/// <GroupHierarchy>
///   <Rule path="transportation/public-transit/light-rail">
///     <Filter>[railway] = 'light_rail'</Filter>
///   </Rule>
///   ...
/// </GroupHierarchy>
/// ```
/// into an ordered rule list, reusing Mapnik's own XML reader
/// (`mapnik::read_xml`/`xml_node`) and CSS-like filter expression parser
/// (`mapnik::parse_expression`) -- the same machinery Mapnik uses to load
/// its own style XML `<Rule><Filter>` blocks, so no new parser or dependency
/// is needed here. Throws `mapnik::datasource_exception` (via `read_xml`'s
/// own exceptions, or `node_not_found`/`attribute_not_found` on a malformed
/// file) on failure.
std::vector<group_rule> load_group_hierarchy(std::string const& path);

/// Attribute names referenced by any rule's filter (via
/// `mapnik::expression_attributes`) -- fed back into the datasource's
/// always-fetched key set, so a `<Filter>` can reference any tag without the
/// caller also having to edit `tags=`/the active style, mirroring how
/// `correlation_tags()` already guarantees `railway`/`highway`/etc. are
/// fetched regardless of the style.
std::set<std::string> referenced_attributes(std::vector<group_rule> const& rules);

/// Evaluates `rules` against `feature` in order, returning the first match's
/// `(path, rank)`. `std::nullopt` when the feature matches no rule (or
/// `rules` is empty).
std::optional<std::pair<std::string, int32_t>> resolve_group(
    std::vector<group_rule> const& rules, mapnik::feature_impl const& feature);

} // namespace osmflat

#endif // OSMFLAT_GROUP_HIERARCHY_HPP
