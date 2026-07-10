#ifndef OSMFLAT_FEATURESET_HPP
#define OSMFLAT_FEATURESET_HPP

#include <mapnik/feature.hpp>
#include <mapnik/featureset.hpp>

#include <memory>
#include <set>
#include <string>
#include <vector>

#include "osmflat_archive.hpp"
#include "osmflat_dump_sink.hpp"

namespace osmflat {

/// Mapnik featureset that pulls features one at a time out of an osmflat
/// `feature_set` query result, building mapnik geometries (points/lines) and
/// attaching attributes.
///
/// Because attributes are query-driven, the schema is *fixed* for the whole
/// query: the synthetic keys (`osm_id`, `osm_type`, `is_closed`) plus the
/// requested tag `keys`. So a single shared `context` is safe, and every
/// feature is completed against it — absent tags are filled with `value_null`,
/// which keeps style filters from throwing "Key does not exist".
///
/// `keys` may be wider than what the mapnik context exposes: when a
/// correlation dump is active, `keys[0, style_key_count)` are the
/// style-requested tags (put onto the mapnik feature as before), while any
/// remaining keys are dump-only naming/classifying tags — fetched from the
/// archive for the dump record but never exposed to the style, so the mapnik
/// attribute schema is unaffected by whether a dump is running.
class osmflat_featureset : public mapnik::Featureset
{
public:
    osmflat_featureset(feature_set&& fs, std::vector<std::string> keys,
                       std::size_t style_key_count, std::vector<std::string> name_langs,
                       std::set<std::string> numeric_keys,
                       std::shared_ptr<dump_sink> dump);
    mapnik::feature_ptr next() override;

private:
    feature_set fs_;
    std::vector<std::string> keys_;   // requested tag names, aligned to attr(i)
    std::size_t style_key_count_;     // keys_[0, style_key_count_) go on the mapnik feature

    // Language preference, in priority order; empty unless the style
    // requested [name] *and* the datasource's `name_lang` param was set (see
    // osmflat_datasource::features()) -- otherwise this stays empty and
    // "name" resolution is left untouched, same as before this feature
    // existed. A non-"_" token N corresponds, in order, to one of the
    // "name:<lang>" keys osmflat_datasource appended at
    // keys_[style_key_count_, ...); "_" means the plain "name" tag (already
    // fetched as a style key) rather than a fetched candidate. Mirrors
    // mod_tile's `coalesce(tags->'name:<lang>', ..., name)` PostGIS rewrite:
    // the first token (in order) with a non-empty value wins, and if no
    // token matches, "name" resolves to null -- exactly like an unmatched
    // SQL coalesce -- rather than silently keeping the plain tag. Callers
    // that want a guaranteed fallback append "_" as the last token.
    std::vector<std::string> name_langs_;
    std::set<std::string> numeric_keys_;  // subset of keys_ to coerce to numbers
    mapnik::context_ptr ctx_;
    mapnik::value_integer feature_id_ = 0;   // mapnik FID (unique within query)
    std::shared_ptr<dump_sink> dump_;

    void write_dump_record();
};

} // namespace osmflat

#endif // OSMFLAT_FEATURESET_HPP
