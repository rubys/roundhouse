class SurveysController < ActionController::Base
  def index
    buffer = Zip::OutputStream.write_buffer do |zip|
      zip.put_next_entry("nested/synthetic.txt")
      zip.write("ZIP payload\n")
    end
    stream = Zip::InputStream.new(buffer)
    entry = stream.get_next_entry
    result = [entry.name, stream.read, stream.get_next_entry]
    stream.close
    result
  end
end
