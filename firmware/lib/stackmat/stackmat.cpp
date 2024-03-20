#include <Arduino.h>
#include "stackmat.h"

Stackmat::Stackmat() {}

void Stackmat::begin(Stream *_serial) {
    serial = _serial;
}

void Stackmat::loop() {
  String data;
  while (serial->available() > 9) {
    data = ReadStackmatString();

    if (data.length() >= 8) {
      ParseTimerData(data);
    }
  }
}

uint8_t Stackmat::displayMinutes() {
  return timerTime / 60000;
}

uint8_t Stackmat::displaySeconds() {
  return (timerTime % 60000) / 1000;
}

uint16_t Stackmat::displayMilliseconds() {
  return timerTime % 1000;
}

StackmatTimerState Stackmat::state() {
    return currentTimerState;
}

int Stackmat::time() {
    return timerTime;
}

bool Stackmat::connected() {
    return millis() - lastUpdated < STACKMAT_TIMER_TIMEOUT;
}

String Stackmat::ReadStackmatString() {
  unsigned long startTime = millis();
  String tmp;

  while (millis() - startTime < 1000) {
    if (serial->available() > 0) {
      char c = serial->read();

      if ((int)c == 0) {
        return tmp;
      }

      if (c == '\r') {
        return tmp;
      }

      tmp += c;
      startTime = millis();
    }
  }

  return "";
}

bool Stackmat::ParseTimerData(String data) {
  StackmatTimerState state = (StackmatTimerState)data[0];
  if (data[0] != 'I' && data[0] != ' ' && data[0] != 'S') {
    state = ST_Unknown;
  }

  int minutes = data.substring(1, 2).toInt();
  int seconds = data.substring(2, 4).toInt();
  int ms = data.substring(4, 7).toInt();
  int cheksum = (int)data[7];

  int totalMs = ms + (seconds * 1000) + (minutes * 60 * 1000);
  int sum = 64;
  for (int i = 0; i < 7; i++) {
    sum += data.substring(i, i + 1).toInt();
  }

  if (sum != cheksum) {
    return false;
  }

  if (totalMs > 0 && state == ST_Reset) {
    state = ST_Stopped;
  }

  currentTimerState = state;
  lastUpdated = millis();
  timerTime = totalMs;

  return true;
}