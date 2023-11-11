#include <Arduino.h>
#include <SoftwareSerial.h>
#include <EEPROM.h>

static const long STACKMAT_TIMER_BAUD_RATE = 1200;
static const long STACKMAT_TIMER_TIMEOUT = 1000;

enum StackmatTimerState {
  ST_Unknown = 0,
  ST_Reset = 'I',
  ST_Running = ' ',
  ST_Stopped = 'S'
};

SoftwareSerial stackmatSerial(1, 255, true);

StackmatTimerState currentState = ST_Reset;
StackmatTimerState lastState = ST_Unknown;
unsigned long lastUpdated = 0;
unsigned long timerTime = 0;
bool isConnected = false;

void setup() {
  Serial.begin(19200);
  stackmatSerial.begin(STACKMAT_TIMER_BAUD_RATE);
}

void loop() {
  String data;

  while (stackmatSerial.available() > 9) {
    data = readStackmatString();
  }

  if (data.length() >= 8) {
    ParseTimerData(data);
  }

  isConnected = millis() - lastUpdated < STACKMAT_TIMER_TIMEOUT;

  if (!isConnected) {
    Serial.println("Timer is disconnected! Make sure it is connected and turned on.");
    //NVIC_SystemReset();

    delay(100);
  }

  if (currentState != lastState) {
    switch (currentState) {
      case ST_Stopped:
        Serial.printf("FINISH! Final time is %i:%02i.%03i!\n", GetDisplayMinutes(), GetDisplaySeconds(), GetDisplayMilliseconds());
        break;
      case ST_Reset:
        Serial.println("Timer is reset!");
        break;
      case ST_Running:
        Serial.println("GO!");
        break;
      default:
        break;
    }
  }

  if (currentState == ST_Running) {
    Serial.printf("%i:%02i.%03i\n", GetInterpolatedDisplayMinutes(), GetInterpolatedDisplaySeconds(), GetInterpolatedDisplayMilliseconds());
  }

  lastState = currentState;
  delay(10);
}

String readStackmatString() {
  unsigned long startTime = millis();
  String tmp;

  while (millis() - startTime < 1000) {
    if (stackmatSerial.available() > 0) {
      char c = stackmatSerial.read();
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

bool ParseTimerData(String data) {
  unsigned long preMillis = millis();

  StackmatTimerState state = (StackmatTimerState)data[0];
  int minutes = data.substring(1, 2).toInt();
  int seconds = data.substring(2, 4).toInt();
  int ms = data.substring(4, 7).toInt();
  int cheksum = (int)data[7];

  unsigned long totalMs = ms + (seconds * 1000) + (minutes * 60 * 1000);
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

  currentState = state;
  lastUpdated = preMillis;
  timerTime = totalMs;

  return true;
}

uint32_t GetInterpolatedTime() {
  if (currentState != ST_Running) {
    return timerTime;
  }

  return timerTime + (millis() - lastUpdated);
}


uint8_t GetDisplayMinutes() {
  return timerTime / 60000;
}

uint8_t GetInterpolatedDisplayMinutes() {
  return GetInterpolatedTime() / 60000;
}

uint8_t GetDisplaySeconds() {
  return (timerTime - ((timerTime / 60000) * 60000)) / 1000;
}

uint8_t GetInterpolatedDisplaySeconds() {
  uint32_t interpolatedTime = GetInterpolatedTime();
  return (interpolatedTime - ((interpolatedTime / 60000) * 60000)) / 1000;
}

uint16_t GetDisplayMilliseconds() {
  uint32_t time = timerTime;
  time -= ((time / 60000) * 60000);
  time -= ((time / 1000) * 1000);
  return time;
}

uint16_t GetInterpolatedDisplayMilliseconds() {
  uint32_t interpolatedTime = GetInterpolatedTime();
  interpolatedTime -= ((interpolatedTime / 60000) * 60000);
  interpolatedTime -= ((interpolatedTime / 1000) * 1000);
  return interpolatedTime;
}
