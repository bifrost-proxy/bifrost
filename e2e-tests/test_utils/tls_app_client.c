#include <curl/curl.h>
#include <stdio.h>

int main(int argc, char **argv) {
    if (argc != 4) {
        fprintf(stderr, "usage: %s PROXY CA_FILE URL\n", argv[0]);
        return 1;
    }
    CURL *client = curl_easy_init();
    if (!client) {
        return 1;
    }
    char error[CURL_ERROR_SIZE] = {0};
    curl_easy_setopt(client, CURLOPT_ERRORBUFFER, error);
    curl_easy_setopt(client, CURLOPT_PROXY, argv[1]);
    curl_easy_setopt(client, CURLOPT_CAINFO, argv[2]);
    curl_easy_setopt(client, CURLOPT_URL, argv[3]);
    curl_easy_setopt(client, CURLOPT_HTTP_VERSION, CURL_HTTP_VERSION_1_1);
    curl_easy_setopt(client, CURLOPT_FAILONERROR, 1L);
    curl_easy_setopt(client, CURLOPT_CONNECTTIMEOUT, 5L);
    curl_easy_setopt(client, CURLOPT_TIMEOUT, 15L);
    CURLcode result = curl_easy_perform(client);
    if (result != CURLE_OK) {
        fprintf(stderr, "%s: %s\n", curl_easy_strerror(result), error);
    }
    curl_easy_cleanup(client);
    return (int)result;
}
