// A short two-note chime, made in Web Audio rather than shipped as a file.
// Silent where the web view has no audio.

/** Rising fifth: E6 then B6. */
const NOTES = [1318.5, 1975.5];
const STEP = 0.11;
const LENGTH = 0.32;
const VOLUME = 0.12;

let audio: AudioContext | null = null;

export function playChime(): void {
  const Audio = typeof window === "undefined" ? undefined : window.AudioContext;
  if (!Audio) return;
  try {
    audio ??= new Audio();
    void audio.resume();
    const start = audio.currentTime;
    NOTES.forEach((freq, i) => {
      const at = start + i * STEP;
      const tone = audio!.createOscillator();
      const gain = audio!.createGain();
      tone.type = "sine";
      tone.frequency.value = freq;
      gain.gain.setValueAtTime(0, at);
      gain.gain.linearRampToValueAtTime(VOLUME, at + 0.01);
      gain.gain.exponentialRampToValueAtTime(0.0001, at + LENGTH);
      tone.connect(gain).connect(audio!.destination);
      tone.start(at);
      tone.stop(at + LENGTH);
    });
  } catch {
    // A sound is a nicety; never let it break the window.
  }
}
