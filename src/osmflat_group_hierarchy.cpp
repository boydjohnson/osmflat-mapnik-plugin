#include "osmflat_group_hierarchy.hpp"

#include <mapnik/attribute_collector.hpp>
#include <mapnik/expression_evaluator.hpp>
#include <mapnik/util/variant.hpp>
#include <mapnik/xml_loader.hpp>
#include <mapnik/xml_node.hpp>
#include <mapnik/xml_tree.hpp>

namespace osmflat {

std::vector<group_rule> load_group_hierarchy(std::string const& path)
{
    mapnik::xml_tree tree;
    mapnik::read_xml(path, tree.root());
    mapnik::xml_node const& root = tree.root().get_child("GroupHierarchy");

    std::vector<group_rule> rules;
    int32_t rank = 0;
    for (mapnik::xml_node const& rule_node : root) {
        if (!rule_node.is("Rule")) {
            continue;
        }
        std::string rule_path = rule_node.get_attr<std::string>("path");
        std::string filter_text = rule_node.get_child("Filter").get_text();
        rules.push_back(group_rule{mapnik::parse_expression(filter_text), rule_path, rank});
        ++rank;
    }
    return rules;
}

std::set<std::string> referenced_attributes(std::vector<group_rule> const& rules)
{
    std::set<std::string> names;
    mapnik::expression_attributes<std::set<std::string>> collector(names);
    for (auto const& rule : rules) {
        mapnik::util::apply_visitor(collector, *rule.filter);
    }
    return names;
}

std::optional<std::pair<std::string, int32_t>> resolve_group(
    std::vector<group_rule> const& rules, mapnik::feature_impl const& feature)
{
    static const mapnik::attributes no_vars;
    for (auto const& rule : rules) {
        mapnik::value result = mapnik::util::apply_visitor(
            mapnik::evaluate<mapnik::feature_impl, mapnik::value, mapnik::attributes>(feature, no_vars),
            *rule.filter);
        if (result.to_bool()) {
            return std::make_pair(rule.path, rule.rank);
        }
    }
    return std::nullopt;
}

} // namespace osmflat
