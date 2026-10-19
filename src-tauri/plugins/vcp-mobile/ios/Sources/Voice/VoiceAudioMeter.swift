import Foundation

/// The same 512-sample Hann FFT and perceptual display scale as Android's
/// VoiceAudioMeter. Meter gain never changes samples supplied to the model.
final class VoiceAudioMeter {
    private var real = [Double](repeating: 0, count: 512)
    private var imaginary = [Double](repeating: 0, count: 512)
    private let window = (0..<512).map { 0.5 - 0.5 * cos(2 * Double.pi * Double($0) / 511) }
    private let ranges: [Range<Int>] = (0..<32).map { index in
        let low = min(255, max(1, Int(floor(80 * pow(100, Double(index) / 32) / 31.25))))
        let high = min(256, max(low + 1, Int(ceil(80 * pow(100, Double(index + 1) / 32) / 31.25))))
        return low..<high
    }

    func measure(_ samples: UnsafePointer<Float>) -> (level: Float, bands: [Float]) {
        var energy = 0.0
        for index in 0..<512 {
            let value = Double(samples[index])
            energy += value * value
            real[index] = value * window[index]
            imaginary[index] = 0
        }
        var j = 0
        for i in 1..<512 {
            var bit = 256
            while j & bit != 0 { j ^= bit; bit >>= 1 }
            j ^= bit
            if i < j { real.swapAt(i, j) }
        }
        var length = 2
        while length <= 512 {
            let angle = -2 * Double.pi / Double(length)
            let stepReal = cos(angle)
            let stepImaginary = sin(angle)
            var base = 0
            while base < 512 {
                var twiddleReal = 1.0
                var twiddleImaginary = 0.0
                for offset in 0..<(length / 2) {
                    let left = base + offset
                    let right = left + length / 2
                    let oddReal = real[right] * twiddleReal - imaginary[right] * twiddleImaginary
                    let oddImaginary = real[right] * twiddleImaginary + imaginary[right] * twiddleReal
                    real[right] = real[left] - oddReal
                    imaginary[right] = imaginary[left] - oddImaginary
                    real[left] += oddReal
                    imaginary[left] += oddImaginary
                    let nextReal = twiddleReal * stepReal - twiddleImaginary * stepImaginary
                    twiddleImaginary = twiddleReal * stepImaginary + twiddleImaginary * stepReal
                    twiddleReal = nextReal
                }
                base += length
            }
            length *= 2
        }
        let rms = sqrt(energy / 512)
        let level = min(1, pow(max(0, rms - 0.002) / 0.06, 0.65))
        let bands = ranges.map { range -> Float in
            var peak = 0.0
            for bin in range {
                peak = max(peak, sqrt(real[bin] * real[bin] + imaginary[bin] * imaginary[bin]) / 128)
            }
            let decibels = 20 * log10(max(peak, 1e-7))
            return Float(min(1, max(0, (decibels + 70) / 55)) * sqrt(level))
        }
        return (Float(level), bands)
    }
}
