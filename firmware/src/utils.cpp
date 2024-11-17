#include "utils.hpp"

#include "driver/rtc_io.h"
#include "globals.hpp"
#include "ws_logger.h"

float batteryVoltageOffset = 0.0;

void lightSleep(gpio_num_t gpio, int level) {
  Logger.println("Going into light sleep...");
  Serial.flush();
  Logger.loop(true);
  webSocket.loop();
  webSocket.disconnect();

  rtc_gpio_hold_en(gpio);
  esp_sleep_enable_ext0_wakeup(gpio, level);
  esp_light_sleep_start();

  Logger.println("Waked up from light sleep...");
}

uint16_t analogReadMax(int pin, int c, int delayMs) {
  uint16_t v = 0;
  for (int i = 0; i < c; i++) {
    v = max(analogRead(pin), v);
    delay(delayMs);
  }

  return v;
}

float voltageToPercentage(float voltage) {
  voltage = constrain(voltage, MIN_VOLTAGE, MAX_VOLTAGE);

  return ((voltage - MIN_VOLTAGE) / (MAX_VOLTAGE - MIN_VOLTAGE)) * 100;
}

float readBatteryVoltage(int pin, int delayMs, bool offset) {
  float val = analogReadMax(pin, 10, delayMs);
  float voltage = val * READ_OFFSET * (V_REF / MAX_ADC) * ((R1 + R2) / R2);

  return voltage + (offset ? batteryVoltageOffset : 0);
}

void sendBatteryStats(float level, float voltage) {
  JsonDocument doc;
  doc["data"]["battery"]["level"] = level;
  doc["data"]["battery"]["voltage"] = voltage;

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);
}

void sendAddDevice() {
  JsonDocument doc;
  doc["data"]["add"]["firmware"] = ADD_DEVICE_FIRMWARE_TYPE;

  String json;
  serializeJson(doc, json);
  webSocket.sendTXT(json);
}

unsigned long epochBase = 0;
unsigned long getEpoch() {
  if (epochBase == 0)
    return 0;
  return epochBase + (millis() / 1000);
}

void clearDisplay(uint8_t filler) {
  digitalWrite(DIS_STCP, LOW);

  for (int i = 0; i < DIS_LENGTH; i++) {
    shiftOut(DIS_DS, DIS_SHCP, LSBFIRST, filler);
  }

  digitalWrite(DIS_STCP, HIGH);
}

int decDigits[10] = {215, 132, 203, 206, 156, 94, 95, 196, 223, 222};
int dotMod = 32;
void displayStr(String str) {
  digitalWrite(DIS_STCP, LOW);

  int pos = 0;
  for (int i = 0; i < DIS_LENGTH - str.length(); i++) {
    shiftOut(DIS_DS, DIS_SHCP, LSBFIRST, 255);
    pos++;
  }

  for (int i = 0; i < str.length(); i++) {
    bool showDot = pos == 0 || pos == 2; // show '.' and ':'

    int digit = str[i] - '0';
    shiftOut(DIS_DS, DIS_SHCP, LSBFIRST,
             ~decDigits[digit] ^ (showDot ? dotMod : 0));
    pos++;
  }

  digitalWrite(DIS_STCP, HIGH);
}

String displayTime(uint8_t m, uint8_t s, uint16_t ms, bool special) {
  String tmp = "";
  if (m > 0) {
    tmp += m;
    if (special)
      tmp += ":";

    char sBuff[6];
    sprintf(sBuff, "%02d", s);
    tmp += String(sBuff);
  } else {
    tmp += s;
  }

  char msBuff[6];
  sprintf(msBuff, "%03d", ms);

  if (special)
    tmp += ".";
  tmp += String(msBuff);
  return tmp;
}
