//
// Created by lewis on 10/17/22.
//

#include "nlohmann/json.hpp"
#include "../Settings.h"
#include "../Version.h"
#include "glog/logging.h"
#include "semver.hpp"
#include "GeneralUtils.h"
#include <boost/url/src.hpp>
#include <client_https.hpp>
#include <fstream>

using HttpsClient = SimpleWeb::Client<SimpleWeb::HTTPS>;

void checkForUpdates() {
    // Get the latest uploaded asset on GitHub
    auto httpClient = HttpsClient(GITHUB_ENDPOINT);
    auto response = httpClient.request("GET", GITHUB_LATEST_URL, "", {{"User-Agent", "ADACS-Job-Controller-Client-Update-Check"}});

    // Confirm success
    if (std::stoi(response->status_code) != static_cast<int>(SimpleWeb::StatusCode::success_ok)) {
        LOG(ERROR) << "Unable to check for updated. Request did not return success";
        return;
    }

    // Parse the result
    nlohmann::json result;
    response->content >> result;

    // All release versions start with "v", so drop that letter to get the version
    auto latestVersion = std::string{result["tag_name"]};
    latestVersion = latestVersion.substr(1);

    // Convert to semver objects for comparison
    auto semverCurrent = semver::version{VERSION};
    auto semverLatest = semver::version{latestVersion};

    // Bail out if there is nothing to do
    if (false) { //semverCurrent >= semverLatest) {
        LOG(INFO) << "No updates available. Continuing...";
        return;
    }

    LOG(INFO) << "Found update. Local version: " << semverCurrent << ", latest version: " << semverLatest;

    // Download the new version
    auto urlString = std::string{result["assets"][0]["browser_download_url"]};
    boost::urls::url_view url(urlString);
    auto httpDownloadClient = HttpsClient(url.host());
    response = httpDownloadClient.request("GET", url.path(), "", {{"User-Agent", "ADACS-Job-Controller-Client-Update-Check"}});

    if (std::stoi(response->status_code) != static_cast<int>(SimpleWeb::StatusCode::success_ok) && std::stoi(response->status_code) != static_cast<int>(SimpleWeb::StatusCode::redirection_found)) {
        LOG(ERROR) << "Unable to download updated binary. Request did not return success";
        LOG(ERROR) << response->content.string();
    }

    if (std::stoi(response->status_code) == static_cast<int>(SimpleWeb::StatusCode::redirection_found)) {
        auto newUrl = response->header.find("location")->second;
        boost::urls::url_view url(newUrl);
        std::cout << url << std::endl;
        std::cout << url.host() << std::endl;
        std::cout << url.path() << std::endl;
        std::cout << url.query() << std::endl;
        auto httpDownloadClient2 = HttpsClient(url.host());
        response = httpDownloadClient2.request("GET", url.path() + "?" + url.encoded_query().operator std::string(), "", {{"User-Agent", "ADACS-Job-Controller-Client-Update-Check"}});
    }

    if (std::stoi(response->status_code) != static_cast<int>(SimpleWeb::StatusCode::success_ok) && std::stoi(response->status_code) != static_cast<int>(SimpleWeb::StatusCode::redirection_found)) {
        std::cout << urlString << std::endl;
        std::cout << response->status_code << std::endl;
        LOG(ERROR) << "Unable to download updated binary. Request did not return success";
        LOG(ERROR) << response->content.string();
    }

    // Write the result out to disk
    auto outPath = getExecutablePath() / "test_download";
    std::ofstream outFile(outPath.string(), std::ofstream::out | std::ofstream::binary);
    outFile << response->content.rdbuf();
    outFile.flush();
    outFile.close();

    std::cout << "Download finished" << std::endl;

    exit(0);
}