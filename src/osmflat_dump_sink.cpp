#include "osmflat_dump_sink.hpp"

#include <iomanip>
#include <ios>
#include <sstream>
#include <stdexcept>

namespace osmflat {

namespace {

void write_escaped(std::ostream& os, const std::string& s)
{
    os << '"';
    for (char c : s) {
        switch (c) {
            case '"': os << "\\\""; break;
            case '\\': os << "\\\\"; break;
            case '\n': os << "\\n"; break;
            case '\r': os << "\\r"; break;
            case '\t': os << "\\t"; break;
            default:
                if (static_cast<unsigned char>(c) < 0x20) {
                    os << "\\u" << std::hex << std::setw(4) << std::setfill('0')
                       << static_cast<int>(static_cast<unsigned char>(c))
                       << std::dec << std::setfill(' ');
                } else {
                    os << c;
                }
        }
    }
    os << '"';
}

void write_point(std::ostream& os, const std::pair<double, double>& p)
{
    os << '[' << p.first << ',' << p.second << ']';
}

void write_ring(std::ostream& os, const std::vector<std::pair<double, double>>& ring)
{
    os << '[';
    for (std::size_t i = 0; i < ring.size(); ++i) {
        if (i) { os << ','; }
        write_point(os, ring[i]);
    }
    os << ']';
}

void write_geometry(std::ostream& os, const dump_record& rec)
{
    os << "{\"type\":";
    write_escaped(os, rec.geom_type);
    os << ",\"coordinates\":";

    if (rec.geom_type == "Point") {
        if (!rec.ring.empty()) {
            write_point(os, rec.ring.front());
        } else {
            os << "[]";
        }
    } else if (rec.geom_type == "MultiPolygon") {
        os << '[';
        for (std::size_t p = 0; p < rec.polygons.size(); ++p) {
            if (p) { os << ','; }
            os << '[';
            for (std::size_t r = 0; r < rec.polygons[p].size(); ++r) {
                if (r) { os << ','; }
                write_ring(os, rec.polygons[p][r]);
            }
            os << ']';
        }
        os << ']';
    } else { // LineString (default/fallback shape)
        write_ring(os, rec.ring);
    }

    os << '}';
}

} // namespace

dump_sink::dump_sink(const std::string& path)
    : out_(path, std::ios::out | std::ios::app)
{
    if (!out_) {
        throw std::runtime_error("osmflat: failed to open dump file '" + path + "'");
    }
}

void dump_sink::write(const dump_record& rec)
{
    std::ostringstream line;
    line << std::setprecision(11);

    line << "{\"type\":\"Feature\",\"properties\":{";
    line << "\"osm_type\":";
    write_escaped(line, rec.osm_type);

    line << ",\"osm_id\":";
    if (rec.has_osm_id) {
        line << rec.osm_id;
    } else {
        line << "null";
    }

    line << ",\"z_order\":" << rec.z_order;
    line << ",\"way_area\":" << rec.way_area;
    line << ",\"is_closed\":" << (rec.is_closed ? "true" : "false");

    for (auto const& kv : rec.tags) {
        line << ',';
        write_escaped(line, kv.first);
        line << ':';
        write_escaped(line, kv.second);
    }
    line << "},\"geometry\":";
    write_geometry(line, rec);
    line << "}\n";

    std::lock_guard<std::mutex> lock(mutex_);
    out_ << line.str();
    out_.flush(); // offline debugging artifact: durability over throughput
}

} // namespace osmflat
