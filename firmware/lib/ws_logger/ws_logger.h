#ifndef __WS_LOGGER_H__
#define __WS_LOGGER_H__

#include <WebSocketsClient.h>
#include <ArduinoJson.h>
#include <vector>

struct logData {
    unsigned long millis;
    String msg;
};

class WsLogger : public Print {
  using Print::print;
  
  public:
    void begin(HardwareSerial* serial, unsigned long _sendInterval = 5000);
    size_t write(uint8_t val) override;
    size_t write(const uint8_t *buffer, size_t size) override;
    void setWsClient(WebSocketsClient* _wsClient);
    void setMaxSize(int logsSize);
    void loop();

  private:
    HardwareSerial* _serial;
    WebSocketsClient* wsClient;

    unsigned long lastSent = 0;
    unsigned long sendInterval = 5000;
    int maxLogsSize = 100;
    std::vector<logData> logs;

    void log(String msg);
};

extern WsLogger Logger;

#endif