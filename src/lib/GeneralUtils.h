//
// Created by lewis on 9/4/22.
//

#ifndef ADACS_JOB_CLIENT_GENERALUTILS_H
#define ADACS_JOB_CLIENT_GENERALUTILS_H

#include <boost/filesystem/path.hpp>
#include <nlohmann/json.hpp>

auto getExecutablePath() -> boost::filesystem::path;
auto readClientConfig() -> nlohmann::json;
auto acceptingConnections(uint16_t port) -> bool;
auto generateUUID() -> std::string;


#ifdef BUILD_TESTS
// NOLINTBEGIN
#define EXPOSE_PROPERTY_FOR_TESTING(term) public: auto get##term () { return &term; } auto set##term (typeof(term) value) { term = value; }
#define EXPOSE_PROPERTY_FOR_TESTING_READONLY(term) public: auto get##term () { return &term; }
#define EXPOSE_FUNCTION_FOR_TESTING(term) public: auto call##term () { return term(); }
#define EXPOSE_FUNCTION_FOR_TESTING_ONE_PARAM(term, param) public: auto call##term (param value) { return term(value); }
// NOLINTEND
#else
// Noop
#define EXPOSE_PROPERTY_FOR_TESTING(x)
#define EXPOSE_PROPERTY_FOR_TESTING_READONLY(x)
#define EXPOSE_FUNCTION_FOR_TESTING(x)
#define EXPOSE_FUNCTION_FOR_TESTING_ONE_PARAM(x, y)
#endif


#endif //ADACS_JOB_CLIENT_GENERALUTILS_H
