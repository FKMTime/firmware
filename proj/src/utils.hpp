#include <Arduino.h>
#include <EEPROM.h>
#include <tuple>
#include <stackmat.h>

struct GlobalState {
  // TIMER INTERNALS
  int solveSessionId;
  int finishedSolveTime;
  int timeOffset;
  unsigned long solverCardId;
  String solverName;

  // STACKMAT
  StackmatTimerState lastTiemrState;
  bool stackmatConnected;

  // RFID
  unsigned long lastCardReadTime;
};

String getChipID() {
  uint64_t chipid = ESP.getChipId();
  String chipidStr = String((uint32_t)(chipid >> 32), HEX) + String((uint32_t)chipid, HEX);
  return chipidStr;
}

void writeEEPROMInt(int address, int value) {
  byte lowByte = (value & 0xFF);
  byte highByte = ((value >> 8) & 0xFF);

  EEPROM.write(address, lowByte);
  EEPROM.write(address + 1, highByte);
}

int readEEPROMInt(int address) {
  byte lowByte = EEPROM.read(address);
  byte highByte = EEPROM.read(address + 1);

  return (lowByte | (highByte << 8));
}

std::tuple<string, int, string> parseWsUrl(string url) {
  int port;
  string path;

  if (url.rfind("ws://", 0) == 0) {
    url = url.substr(5);
    port = 80;
  } else if (url.rfind("wss://", 0) == 0) {
    url = url.substr(6);
    port = 443;
  } else {
    std::cout << "Invalid protocol" << std::endl;
  }

  int pathSplitPos = url.find("/");
  if (pathSplitPos == string::npos) {
    pathSplitPos = url.length();
    url = url + "/";
  }

  path = url.substr(pathSplitPos);
  url = url.substr(0, pathSplitPos);

  int portSplitPos = url.rfind(":");
  if (portSplitPos != string::npos) {
    port = stoi(url.substr(portSplitPos + 1));
    url = url.substr(0, portSplitPos);
  }

  return {url, port, path};
}