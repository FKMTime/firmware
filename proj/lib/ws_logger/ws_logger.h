#ifndef __WS_LOGGER_H__
#define __WS_LOGGER_H__

#include <WebSocketsClient.h>
#include <ArduinoJson.h>
#include <vector>

#ifdef ARDUINO_ARCH_ESP32
  #define ESP_ID() ESP.getEfuseMac()
#else
  #define ESP_ID() ESP.getChipId()
#endif

struct logData {
    unsigned long millis;
    String msg;
};

class WsLogger {
  public:
    WsLogger();
    void begin(Stream *_serial, unsigned long _sendInterval);
    void setWsClient(WebSocketsClient *_wsClient);
    void setMaxSize(int logsSize);
    void loop();

    void println(String msg);
    void printf(const char *format, ...);

  private:
    WebSocketsClient *wsClient;
    Stream *serial;

    unsigned long lastSent = 0;
    unsigned long sendInterval = 5000;
    int maxLogsSize = 100;
    std::vector<logData> logs;

    void log(String msg);
};

extern WsLogger Logger;

#endif