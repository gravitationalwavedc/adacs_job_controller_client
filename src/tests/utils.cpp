//
// Created by lewis on 9/6/22.
//

#include <random>
#include "utils.h"

std::random_device rnd_device;
std::shared_ptr<std::mt19937> rng = nullptr; // NOLINT(cppcoreguidelines-avoid-non-const-global-variables)

auto randomInt(uint64_t start, uint64_t end) -> uint64_t {
    if (!rng) {
        rng = std::make_shared<std::mt19937>(rnd_device());
    }

    std::uniform_int_distribution<uint64_t> rng_dist(start, end);
    return rng_dist(*rng);
}

auto generateRandomData(size_t count) -> std::shared_ptr<std::vector<uint8_t>> {
    if (!rng) {
        rng = std::make_shared<std::mt19937>(rnd_device());
    }

    auto count64 = (count / 8) + 1;
    auto result = std::vector<uint64_t>();
    result.reserve(count64);

    std::uniform_int_distribution<uint64_t> dist {0, std::numeric_limits<uint64_t>::max()};

    auto gen = [&dist, rng](){
        return dist(*rng);
    };

    std::generate_n(result.begin(), count64, gen);

    auto pData = reinterpret_cast<uint8_t*>(result.data());
    return std::make_shared<std::vector<uint8_t>>(pData, pData + count);
}